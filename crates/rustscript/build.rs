#[path = "src/bridge_tables_build.rs"]
mod bridge_tables_build;
#[path = "src/builtin_id_build.rs"]
mod builtin_id_build;
#[path = "src/path_id_build.rs"]
mod path_id_build;

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
    // Harvested from the bridge sources so `rust check` can report a method
    // the interpreter lacks.
    let interpreter = std::path::Path::new("src/interpreter");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rows = builtin_id_build::read_table(&interpreter.join("method_names.txt"));
    std::fs::write(
        out_dir.join("builtin_id.rs"),
        builtin_id_build::generate(&rows),
    )
    .expect("write builtin ids");
    let paths = path_id_build::read_paths(&interpreter.join("path_names.txt"));
    std::fs::write(out_dir.join("path_id.rs"), path_id_build::generate(&paths))
        .expect("write path ids");
    let tables = bridge_tables_build::generate(interpreter, &rows);
    std::fs::write(out_dir.join("bridge_tables.rs"), tables).expect("write bridge tables");
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
