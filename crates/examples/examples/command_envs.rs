#!/usr/bin/env rust

// `envs` takes any iterator of pairs, `env` sets 1 variable and `env_remove` unsets 1.
// Every form goes through the same env map on the Command.

use std::collections::HashMap;
use std::process::Command;

fn shell(cmd: &mut Command, script: &str) -> String {
    let out = cmd.args(["-c", script]).output().expect("run sh");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    let pairs = [("DEMO_ONE", "1"), ("DEMO_TWO", "2")];
    let from_iter = shell(
        Command::new("sh").envs(pairs.iter().copied()),
        "echo $DEMO_ONE $DEMO_TWO",
    );
    println!("iter: {from_iter}");

    let from_array = shell(Command::new("sh").envs(pairs), "echo $DEMO_TWO $DEMO_ONE");
    println!("array: {from_array}");

    let mut map = HashMap::new();
    map.insert("DEMO_A".to_string(), "a".to_string());
    map.insert("DEMO_B".to_string(), "b".to_string());
    let from_map = shell(Command::new("sh").envs(&map), "echo $DEMO_A $DEMO_B");
    println!("map: {from_map}");

    let removed = shell(
        Command::new("sh")
            .env("DEMO_KEEP", "kept")
            .envs(pairs)
            .env_remove("DEMO_ONE"),
        "echo ${DEMO_ONE:-unset} $DEMO_TWO $DEMO_KEEP",
    );
    println!("removed: {removed}");
}
