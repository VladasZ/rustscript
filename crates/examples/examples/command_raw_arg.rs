#!/usr/bin/env rust

// `cmd /C` needs its command string untouched. `arg` writes `\"` for a quote inside an
// argument and cmd does not read that escape, so a quoted part with a space falls into 2
// words. `raw_arg` from the Windows `CommandExt` passes the text as it is. Every other system
// goes through `sh -c`, where `arg` is already right.

use std::process::Command;

#[cfg(windows)]
fn shell(cmd: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let mut c = Command::new("cmd");
    // with `/S` cmd strips only the outer pair of quotes
    c.arg("/S").arg("/C").raw_arg(format!("\"{cmd}\""));
    c
}

#[cfg(not(windows))]
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

fn main() {
    // git prints each argument it got in single quotes, so a split argument shows up
    let out = shell(r#"git rev-parse --sq-quote "two words" one"#)
        .output()
        .expect("run git");
    println!("{}", String::from_utf8_lossy(&out.stdout).trim());
    println!("success: {}", out.status.success());
}
