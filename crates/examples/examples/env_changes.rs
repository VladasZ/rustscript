#!/usr/bin/env rust


use std::env;
use std::process::Command;

fn echo_probe() -> String {
    let out = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "echo [%RS_EXAMPLE_PROBE%]"])
            .output()
            .unwrap()
    } else {
        Command::new("sh")
            .args(["-c", "echo [${RS_EXAMPLE_PROBE-%RS_EXAMPLE_PROBE%}]"])
            .output()
            .unwrap()
    };
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    println!("{:?}", env::var("RS_EXAMPLE_PROBE"));
    // SAFETY: the example sets variables before it starts any thread
    unsafe { env::set_var("RS_EXAMPLE_PROBE", "set") };
    println!("{:?} {}", env::var("RS_EXAMPLE_PROBE"), echo_probe());
    let listed = env::vars().any(|(k, v)| k == "RS_EXAMPLE_PROBE" && v == "set");
    println!("{listed}");
    // SAFETY: as above
    unsafe { env::remove_var("RS_EXAMPLE_PROBE") };
    println!("{:?} {}", env::var_os("RS_EXAMPLE_PROBE"), echo_probe());
}
