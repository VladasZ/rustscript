use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Classification {
    Match,
    SemanticMismatch,
    InterpreterUnsupported,
    InterpreterPanic,
    InterpreterCrash,
    InterpreterTimeout,
    NativePanic,
    NativeCrash,
    NativeTimeout,
    RustcRejected,
    RustcTimeout,
}

impl Classification {
    pub fn same_failure(&self, other: &Self) -> bool {
        self == other
    }

    pub fn is_hard_failure(&self) -> bool {
        !matches!(self, Self::Match | Self::InterpreterUnsupported)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub classification: Classification,
    pub compiler: ProcessOutput,
    pub native: ProcessOutput,
    pub interpreted: ProcessOutput,
}

pub struct Runner {
    interpreter: PathBuf,
    timeout: Duration,
}

impl Runner {
    pub fn build(workspace: &Path, timeout_ms: u64) -> Result<Self> {
        let interpreter = match std::env::var_os("RUSTSCRIPT_INTERPRETER") {
            Some(path) => PathBuf::from(path),
            None => {
                let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
                let status = Command::new(cargo)
                    .args(["build", "-p", "run-rs"])
                    .current_dir(workspace)
                    .status()
                    .context("failed to build RustScript")?;
                if !status.success() {
                    bail!("cargo build -p run-rs failed");
                }
                target_dir(workspace).join(executable_name("rust"))
            }
        };
        if !interpreter.is_file() {
            bail!("RustScript binary not found at {}", interpreter.display());
        }
        let interpreter = interpreter
            .canonicalize()
            .context("failed to resolve RustScript binary")?;
        Ok(Self {
            interpreter,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    pub fn run_source(&self, source: &str) -> Result<RunResult> {
        let directory = tempfile::Builder::new()
            .prefix("rustscript-differential-")
            .tempdir()?;
        let source_path = directory.path().join("case.rs");
        let binary_path = directory.path().join(executable_name("case"));
        fs::write(&source_path, source)?;

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiler = run_command(
            Command::new(rustc)
                .args(["--edition", "2024", "-C", "overflow-checks=yes", "-o"])
                .arg(&binary_path)
                .arg(&source_path)
                .current_dir(directory.path()),
            self.timeout,
        )?;
        if compiler.timed_out {
            return Ok(incomplete(Classification::RustcTimeout, compiler));
        }
        if compiler.status != Some(0) {
            return Ok(incomplete(Classification::RustcRejected, compiler));
        }

        let native = run_command(
            Command::new(&binary_path).current_dir(directory.path()),
            self.timeout,
        )?;
        let interpreted = run_command(
            Command::new(&self.interpreter)
                .arg("run")
                .arg(&source_path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1")
                .current_dir(directory.path()),
            self.timeout,
        )?;
        let classification = classify(&native, &interpreted);
        Ok(RunResult {
            classification,
            compiler,
            native,
            interpreted,
        })
    }

    pub fn run_sources(&self, sources: &[String]) -> Result<Vec<RunResult>> {
        if sources.len() <= 1 {
            return sources
                .iter()
                .map(|source| self.run_source(source))
                .collect();
        }

        let directory = tempfile::Builder::new()
            .prefix("rustscript-differential-batch-")
            .tempdir()?;
        let bundle_path = directory.path().join("batch.rs");
        let binary_path = directory.path().join(executable_name("batch"));
        let source_paths = write_batch_sources(directory.path(), sources)?;
        fs::write(&bundle_path, render_native_batch(sources)?)?;

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiler = run_command(
            Command::new(rustc)
                .args(["--edition", "2024", "-C", "overflow-checks=yes", "-o"])
                .arg(&binary_path)
                .arg(&bundle_path)
                .current_dir(directory.path()),
            self.timeout,
        )?;
        if compiler.timed_out || compiler.status != Some(0) {
            return sources
                .iter()
                .map(|source| self.run_source(source))
                .collect();
        }

        source_paths
            .iter()
            .enumerate()
            .map(|(index, source_path)| {
                let native = run_command(
                    Command::new(&binary_path)
                        .env("RUSTSCRIPT_DIFFERENTIAL_CASE", index.to_string())
                        .current_dir(directory.path()),
                    self.timeout,
                )?;
                let interpreted = self.run_interpreted(source_path, directory.path())?;
                let classification = classify(&native, &interpreted);
                Ok(RunResult {
                    classification,
                    compiler: compiler.clone(),
                    native,
                    interpreted,
                })
            })
            .collect()
    }

    fn run_interpreted(&self, source_path: &Path, directory: &Path) -> Result<ProcessOutput> {
        run_command(
            Command::new(&self.interpreter)
                .arg("run")
                .arg(source_path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1")
                .current_dir(directory),
            self.timeout,
        )
    }
}

fn write_batch_sources(directory: &Path, sources: &[String]) -> Result<Vec<PathBuf>> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let path = directory.join(format!("case_{index}.rs"));
            fs::write(&path, source)?;
            Ok(path)
        })
        .collect()
}

fn render_native_batch(sources: &[String]) -> Result<String> {
    let mut bundle = String::new();
    for (index, source) in sources.iter().enumerate() {
        let module_source = source.replacen("fn main() {", "pub fn run() {", 1);
        if module_source == *source {
            bail!("generated source {index} has no main function");
        }
        bundle.push_str(&format!("mod case_{index} {{\n{module_source}\n}}\n\n"));
    }
    bundle.push_str(
        r#"fn main() {
    let index = std::env::var("RUSTSCRIPT_DIFFERENTIAL_CASE")
        .expect("missing case index")
        .parse::<usize>()
        .expect("invalid case index");
    match index {
"#,
    );
    for index in 0..sources.len() {
        bundle.push_str(&format!("        {index} => case_{index}::run(),\n"));
    }
    bundle.push_str(
        r#"        _ => panic!("case index out of range"),
    }
}
"#,
    );
    Ok(bundle)
}

fn target_dir(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        }
        None => workspace.join("target"),
    }
    .join("debug")
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn incomplete(classification: Classification, compiler: ProcessOutput) -> RunResult {
    let empty = ProcessOutput {
        status: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    RunResult {
        classification,
        compiler,
        native: empty.clone(),
        interpreted: empty,
    }
}

fn classify(native: &ProcessOutput, interpreted: &ProcessOutput) -> Classification {
    if native.timed_out {
        return Classification::NativeTimeout;
    }
    if native.status != Some(0) {
        return if native.status == Some(101) {
            Classification::NativePanic
        } else {
            Classification::NativeCrash
        };
    }
    if interpreted.timed_out {
        return Classification::InterpreterTimeout;
    }
    if interpreted.status == Some(101) {
        return Classification::InterpreterPanic;
    }
    if interpreted.status != Some(0) {
        let error = interpreted.stderr.to_ascii_lowercase();
        return if error.contains("unsupported")
            || error.contains("not supported")
            || error.contains("not implemented by the interpreter")
        {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterCrash
        };
    }
    if native.stdout == interpreted.stdout && native.stderr == interpreted.stderr {
        Classification::Match
    } else {
        Classification::SemanticMismatch
    }
}

fn run_command(command: &mut Command, timeout: Duration) -> Result<ProcessOutput> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {:?}", command.get_program()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("child stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("child stderr was not captured"))?;
    let stdout_reader = spawn(move || read_pipe(stdout));
    let stderr_reader = spawn(move || read_pipe(stderr));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            child.kill().context("failed to kill timed out process")?;
            break (child.wait()?, true);
        }
        sleep(Duration::from_millis(5));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("child stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("child stderr reader panicked"))??;
    Ok(ProcessOutput {
        status: status.code(),
        stdout,
        stderr,
        timed_out,
    })
}

fn read_pipe(mut pipe: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(status: i32, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            status: Some(status),
            stdout: String::new(),
            stderr: stderr.to_string(),
            timed_out: false,
        }
    }

    #[test]
    fn unsupported_errors_are_gaps() {
        assert_eq!(
            classify(&output(0, ""), &output(1, "unsupported item: macro")),
            Classification::InterpreterUnsupported
        );
    }

    #[test]
    fn different_output_is_a_semantic_failure() {
        let native = ProcessOutput {
            stdout: "one".to_string(),
            ..output(0, "")
        };
        let interpreted = ProcessOutput {
            stdout: "two".to_string(),
            ..output(0, "")
        };
        assert_eq!(
            classify(&native, &interpreted),
            Classification::SemanticMismatch
        );
    }

    #[test]
    fn large_captured_output_does_not_block() -> Result<()> {
        let output = run_command(
            Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "runner::tests::large_output_helper",
                    "--nocapture",
                ])
                .env("RUSTSCRIPT_TEST_LARGE_OUTPUT", "1"),
            Duration::from_secs(10),
        )?;

        assert!(!output.timed_out);
        assert_eq!(output.status, Some(0));
        assert!(output.stderr.len() >= 1024 * 1024);
        Ok(())
    }

    #[test]
    fn large_output_helper() {
        if std::env::var_os("RUSTSCRIPT_TEST_LARGE_OUTPUT").is_some() {
            eprint!("{}", "x".repeat(1024 * 1024));
        }
    }
}
