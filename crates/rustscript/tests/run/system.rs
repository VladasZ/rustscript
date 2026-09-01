use pretty_assertions::assert_eq;

use super::common::{run, run_fail};

#[test]
fn fs_roundtrip() {
    let out = run(r#"
use std::fs;
fn main() -> anyhow::Result<()> {
    let path = "/tmp/rustscript_fs_test.txt";
    fs::write(path, "payload")?;
    let back = fs::read_to_string(path)?;
    println!("{back}");
    Ok(())
}
"#);
    assert_eq!(out, "payload\n");
}

#[test]
fn shell_command() {
    let out = run(r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("echo").arg("hello").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{}", text.trim());
    println!("{}", out.status.success());
    Ok(())
}
"#);
    assert_eq!(out, "hello\ntrue\n");
}

#[test]
fn command_env_remove() {
    let script = if cfg!(windows) {
        r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("cmd")
        .args(["/C", "if defined RUSTSCRIPT_REMOVE_ME (echo present) else (echo absent)"])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}
"#
    } else {
        r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("sh")
        .args(["-c", "if [ -z \"${RUSTSCRIPT_REMOVE_ME+x}\" ]; then echo absent; else echo present; fi"])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}
"#
    };
    // cmd echo ends with CRLF and sh echo with LF, the point is the missing variable
    let expected = if cfg!(windows) {
        "absent\r\n"
    } else {
        "absent\n"
    };
    assert_eq!(run(script), expected);
}

#[test]
fn read_dir_iteration() {
    let out = run(r#"
use std::fs;
fn main() -> anyhow::Result<()> {
    let base = "/tmp/rustscript_readdir_test";
    fs::create_dir_all(base)?;
    fs::write(&format!("{base}/one.txt"), "a")?;
    fs::write(&format!("{base}/two.txt"), "b")?;
    let mut names = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names.sort();
    println!("{:?}", names);
    Ok(())
}
"#);
    assert_eq!(out, "[\"one.txt\", \"two.txt\"]\n");
}

#[test]
fn path_ancestors() {
    let out = run(r#"
use std::path::Path;
fn main() {
    let mut paths = Vec::new();
    for path in Path::new("one/two").ancestors() {
        paths.push(path.display().to_string());
    }
    println!("{:?}", paths);
}
"#);
    assert_eq!(out, "[\"one/two\", \"one\", \"\"]\n");
}

#[test]
fn named_temp_file_and_os_string_into_path() {
    let out = run(r#"
use std::env;
use std::path::PathBuf;
use tempfile::NamedTempFile;
fn main() {
    let file = NamedTempFile::new().unwrap();
    println!("{}", file.path().is_file());
    unsafe { env::set_var("RUSTSCRIPT_PATH_TEST", "/tmp/rustscript-path") };
    let path: PathBuf = env::var_os("RUSTSCRIPT_PATH_TEST").map(Into::into).unwrap();
    println!("{}", path.display());
    unsafe { env::remove_var("RUSTSCRIPT_PATH_TEST") };
}
"#);
    assert_eq!(out, "true\n/tmp/rustscript-path\n");
}

#[test]
fn regex_matching() {
    let out = run(r#"
use regex::Regex;
fn main() -> anyhow::Result<()> {
    let re = Regex::new(r"(\w+)=(\d+)")?;
    let caps = re.captures("port=8080").unwrap();
    println!("{} {}", &caps[1], &caps[2]);
    println!("{}", re.is_match("x=1"));
    let clean = Regex::new(r"\s+")?.replace_all("a  b   c", "-");
    println!("{clean}");
    Ok(())
}
"#);
    assert_eq!(out, "port 8080\ntrue\na-b-c\n");
}

#[test]
fn reqwest_bridge_builds_and_errors_gracefully() {
    // a refused local port must be an `Err`, not a panic
    let out = run(r#"
fn main() {
    let client = reqwest::blocking::Client::new();
    let r = client
        .get("http://127.0.0.1:9/")
        .header("X-Test", "1")
        .send();
    println!("{}", r.is_err());
}
"#);
    assert_eq!(out, "true\n");
}

#[test]
fn std_thread_is_rejected() {
    let err = run_fail(
        r#"
use std::thread;
fn main() {
    let h = thread::spawn(|| 1);
    println!("{}", h.join().unwrap());
}
"#,
    );
    // the coverage gate stops it before the script runs
    assert!(
        err.contains("`std::thread::spawn` is not implemented by the interpreter"),
        "stderr was: {err}"
    );
}
