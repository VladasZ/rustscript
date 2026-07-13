use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use hex::encode;
use sha2::{Digest, Sha256};

use crate::{Meta, NamedHash, Settings};

pub fn gather(
    root: &Path,
    rustscript: &Path,
    fixtures: &[(String, PathBuf)],
    settings: Settings,
) -> Result<Meta> {
    let status = command_line(
        root,
        "git",
        &["status", "--porcelain", "--untracked-files=all"],
    );
    let mut fixture_hashes = Vec::new();
    for (name, path) in fixtures {
        fixture_hashes.push(NamedHash {
            name: name.clone(),
            sha256: hash_file(path)?,
        });
    }
    Ok(Meta {
        date_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        git_commit: command_line(root, "git", &["rev-parse", "HEAD"]),
        git_dirty: !status.is_empty(),
        rustscript_binary: rustscript.display().to_string(),
        rustscript_sha256: hash_file(rustscript)?,
        benchmark_sha256: hash_tree(&root.join("bench"))?,
        fixtures: fixture_hashes,
        rustc: command_line(root, "rustc", &["--version"]),
        cargo: command_line(root, "cargo", &["--version"]),
        node: command_line(root, "node", &["--version"]),
        python: command_line(root, "python3", &["--version"]),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        cpu: if cfg!(target_os = "macos") {
            command_line(root, "sysctl", &["-n", "machdep.cpu.brand_string"])
        } else {
            command_line(root, "uname", &["-p"])
        },
        settings,
    })
}

pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(encode(Sha256::digest(bytes)))
}

fn hash_tree(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    for relative in files {
        let path = root.join(&relative);
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    Ok(encode(digest.finalize()))
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || name == "results"
            || name == "generated"
            || name == "__pycache__"
        {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if !name.starts_with("data_big.") {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn command_line(root: &Path, program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}
