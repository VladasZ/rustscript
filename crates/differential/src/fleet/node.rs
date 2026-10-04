//! One node of the fleet, reached over `ssh`, and the process helpers.

use std::fs::{File, create_dir_all, read_dir, remove_dir_all};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};

/// The folder of the working tree on a node, under the home folder.
pub const REMOTE_DIR: &str = "rustscript-fleet";
/// The folder of the script repo on a node.
pub const REMOTE_SCRIPTS: &str = "rustscript-fleet-scripts";

/// A build or a `rustc` needs about this much memory per job.
const GB_PER_JOB: usize = 2;

const SHEBANG: &str = "#!/usr/bin/env rust";

/// The nodes host game servers and CI runners, so the fleet takes only what they leave.
pub fn low_priority() -> &'static str {
    "nice -n 19 ionice -c 3"
}

/// The same on Windows. The shell of the `ssh` session drops to the idle class and every
/// process it starts stays there.
const WINDOWS_IDLE: &str = "(Get-Process -Id $PID).PriorityClass = 'Idle'";

/// What a node runs. A Windows node is reached over `ssh` into PowerShell and has no `rsync`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Os {
    Linux,
    Windows,
}

pub struct Node {
    pub host: String,
    pub os: Os,
    /// parallel jobs, the cores capped by the free memory
    pub jobs: usize,
    /// what the toolchain check, the copy and the build took
    pub seconds: f64,
}

impl Node {
    /// Installs the pinned toolchain when it is missing, copies the working tree and builds
    /// the interpreter and the harness.
    pub fn prepare(
        root: &Path,
        host: &str,
        os: Os,
        channel: &str,
        scripts: Option<&Path>,
    ) -> Result<Node> {
        let started = Instant::now();
        let mut node = Node {
            host: host.to_string(),
            os,
            jobs: 1,
            seconds: 0.0,
        };
        // both print the cores, then the free memory in GB
        let facts = match os {
            Os::Linux => format!(
                "rustup toolchain list | grep -q '^{channel}-' || rustup toolchain install \
                 {channel} --profile minimal 2>&1; nproc; awk '/MemAvailable/ {{print int($2 / \
                 1048576)}}' /proc/meminfo"
            ),
            Os::Windows => format!(
                "if (-not (rustup toolchain list | Select-String -Quiet '^{channel}-')) {{ rustup \
                 toolchain install {channel} --profile minimal }}; $env:NUMBER_OF_PROCESSORS; \
                 [int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB)"
            ),
        };
        let (ok, text) = node.ssh(&facts)?;
        if !ok {
            bail!("the toolchain check failed: {}", text.trim());
        }
        let mut facts = text
            .lines()
            .rev()
            .filter_map(|line| line.trim().parse::<usize>().ok());
        let (Some(free_gb), Some(cores)) = (facts.next(), facts.next()) else {
            bail!("cannot read the cores and the free memory: {}", text.trim());
        };
        node.jobs = cores.min(free_gb / GB_PER_JOB).max(1);

        node.copy_tree(root, REMOTE_DIR)?;
        if let Some(repo) = scripts {
            node.copy_tree(repo, REMOTE_SCRIPTS)?;
        }
        let (ok, text) = node.ssh(&node.in_tree(&format!(
            "cargo build --release -j {} -p run-rs -p rustscript-differential",
            node.jobs
        )))?;
        if !ok {
            let tail: Vec<&str> = text.lines().rev().take(12).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            bail!("the build failed:\n{}", tail.join("\n"));
        }
        node.seconds = started.elapsed().as_secs_f64();
        Ok(node)
    }

    /// Whether the script ended with success, and what it printed.
    pub fn ssh(&self, script: &str) -> Result<(bool, String)> {
        local(Command::new("ssh").args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &self.host,
            script,
        ]))
    }

    /// The shell line that runs `command` in the working tree at the lowest priority, with the
    /// interpreter of that tree as the one the harness runs. The exit code is the command's.
    pub fn in_tree(&self, command: &str) -> String {
        match self.os {
            Os::Linux => format!(
                "cd {REMOTE_DIR} && RUSTSCRIPT_INTERPRETER=$PWD/target/release/rust {} {command} \
                 2>&1",
                low_priority()
            ),
            Os::Windows => format!(
                "{WINDOWS_IDLE}; cd {REMOTE_DIR}; $env:RUSTSCRIPT_INTERPRETER = \
                 \"$PWD\\target\\release\\rust.exe\"; {command}; exit $LASTEXITCODE"
            ),
        }
    }

    /// The harness binary as the shell of the node names it from the working tree.
    pub fn harness(&self) -> &'static str {
        match self.os {
            Os::Linux => "./target/release/rustscript-differential",
            Os::Windows => ".\\target\\release\\rustscript-differential.exe",
        }
    }

    /// Removes the failing cases of an earlier run, they would look like findings of this one.
    pub fn clear_failures(&self) -> Result<(bool, String)> {
        self.ssh(&match self.os {
            Os::Linux => format!("rm -rf {REMOTE_DIR}/target/rustscript-differential/failures"),
            Os::Windows => format!(
                "Remove-Item -Recurse -Force -ErrorAction SilentlyContinue \
                 {REMOTE_DIR}\\target\\rustscript-differential\\failures; exit 0"
            ),
        })
    }

    /// `rust check` over the scripts. Prints one `FAIL path: reason` line per script that fails.
    pub fn sweep(&self, scripts: &[String]) -> Result<(bool, String)> {
        let list = scripts
            .iter()
            .map(|script| format!("'{script}'"))
            .collect::<Vec<_>>();
        self.ssh(&match self.os {
            Os::Linux => format!(
                "cd {REMOTE_SCRIPTS} && for f in {}; do out=$(RUSTSCRIPT_SKIP_CHECK=1 {} \
                 ~/{REMOTE_DIR}/target/release/rust check \"$f\" 2>&1) || echo \"FAIL $f: $(echo \
                 \"$out\" | head -1)\"; done",
                list.join(" "),
                low_priority()
            ),
            Os::Windows => format!(
                "{WINDOWS_IDLE}; $rust = \"$HOME\\{REMOTE_DIR}\\target\\release\\rust.exe\"; cd \
                 {REMOTE_SCRIPTS}; $env:RUSTSCRIPT_SKIP_CHECK = '1'; foreach ($f in @({})) {{ \
                 $out = & $rust check $f 2>&1; if ($LASTEXITCODE -ne 0) {{ \"FAIL ${{f}}: \
                 $($out | Select-Object -First 1)\" }} }}; exit 0",
                list.join(", ")
            ),
        })
    }

    /// Copies the failing cases of the node next to the local ones.
    pub fn fetch_failures(&self, root: &Path) -> Result<PathBuf> {
        let target = root
            .join("target/rustscript-differential/fleet")
            .join(&self.host);
        let remote = format!(
            "{}:{REMOTE_DIR}/target/rustscript-differential/failures/",
            self.host
        );
        let (ok, text) = match self.os {
            Os::Linux => {
                create_dir_all(&target)?;
                local(Command::new("rsync").args([
                    "-a",
                    "--delete",
                    &remote,
                    &format!("{}/", target.display()),
                ]))?
            }
            Os::Windows => {
                // `scp` adds and never deletes, so the cases of an earlier run go first
                if target.is_dir() {
                    remove_dir_all(&target)?;
                }
                create_dir_all(&target)?;
                local(
                    Command::new("scp")
                        .args(["-q", "-r", &format!("{remote}.")])
                        .arg(&target),
                )?
            }
        };
        if !ok {
            bail!("the copy failed: {}", text.trim());
        }
        Ok(target)
    }

    /// The folder as it is on disk, uncommitted files included. Build output and git history
    /// stay here.
    fn copy_tree(&self, from: &Path, to: &str) -> Result<()> {
        let (ok, text) = match self.os {
            Os::Linux => local(Command::new("rsync").args([
                "-a",
                "--delete",
                "--exclude",
                "target",
                "--exclude",
                ".git",
                "--exclude",
                "node_modules",
                &format!("{}/", from.display()),
                &format!("{}:{to}/", self.host),
            ]))?,
            Os::Windows => self.copy_tree_by_tar(from, to)?,
        };
        if !ok {
            bail!("the copy of {} failed: {}", from.display(), text.trim());
        }
        Ok(())
    }

    /// A Windows node has no `rsync`. Everything but the build output is removed there, then
    /// a `tar` stream brings the tree. `tar` keeps the file times, so cargo rebuilds only what
    /// changed.
    fn copy_tree_by_tar(&self, from: &Path, to: &str) -> Result<(bool, String)> {
        let mut pack = Command::new("tar")
            .args(["-cf", "-", "--exclude", "target", "--exclude", ".git"])
            .args(["--exclude", "node_modules", "-C"])
            .arg(from)
            .arg(".")
            // keeps the `._name` files of macOS out of the stream
            .env("COPYFILE_DISABLE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("cannot run tar")?;
        let stream = pack.stdout.take().context("tar has no output")?;
        let unpacked = local(
            Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &self.host])
                .arg(format!(
                    "New-Item -ItemType Directory -Force {to} | Out-Null; Get-ChildItem -Force \
                     {to} -Exclude target | Remove-Item -Recurse -Force; tar -xf - -C {to}; exit \
                     $LASTEXITCODE"
                ))
                .stdin(stream),
        )?;
        let packed = pack.wait().context("tar did not end")?;
        Ok((unpacked.0 && packed.success(), unpacked.1))
    }
}

/// Runs a command to its end. Whether it succeeded, and stdout followed by stderr.
pub fn local(command: &mut Command) -> Result<(bool, String)> {
    let output = command
        .output()
        .with_context(|| format!("cannot run {}", command.get_program().display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

/// Every file under `repo` whose first line is the interpreter shebang, as a path relative to
/// `repo`.
pub fn script_files(repo: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    walk(repo, repo, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(repo: &Path, dir: &Path, found: &mut Vec<String>) -> Result<()> {
    for entry in read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "target" | ".git" | "node_modules") {
                walk(repo, &path, found)?;
            }
            continue;
        }
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e != "rs")
        {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut first = String::new();
        // a binary file is not a script, a read error just skips it
        if BufReader::new(file).read_line(&mut first).is_ok()
            && first.trim_end() == SHEBANG
            && let Ok(relative) = path.strip_prefix(repo)
        {
            found.push(relative.display().to_string());
        }
    }
    Ok(())
}
