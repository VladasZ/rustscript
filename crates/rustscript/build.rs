#[path = "src/bridge_tables_build.rs"]
mod bridge_tables_build;

use std::env;
use std::path::PathBuf;
use std::process::Command;

use chrono::{SecondsFormat, Utc};

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn git_commit() -> String {
    let Some(commit) = git_output(&["rev-parse", "HEAD"]) else {
        return "unknown".to_string();
    };
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no", "--", "."])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
    if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    }
}

fn main() {
    // Harvest the interpreter's supported method names from the bridge sources
    // so `rust check` can tell a script when it uses one that does not exist.
    let interpreter = std::path::Path::new("src/interpreter");
    let tables = bridge_tables_build::generate(interpreter);
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("bridge_tables.rs");
    std::fs::write(&out, tables).expect("write bridge tables");
    println!("cargo:rerun-if-changed=src/interpreter");
    println!("cargo:rerun-if-changed=src/bridge_tables_build.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");

    if let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    }

    let commit = git_commit();
    let build_time = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=RUSTSCRIPT_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=RUSTSCRIPT_BUILD_TIME={build_time}");
    println!("cargo:rustc-env=RUSTSCRIPT_BUILD_PROFILE={profile}");
}
