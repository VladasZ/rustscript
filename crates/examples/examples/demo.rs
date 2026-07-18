#!/usr/bin/env rust

use anyhow::Result;
use serde::Serialize;
use std::env::consts::OS;
use std::env::temp_dir;
use std::fs;
use std::process::Command;

#[derive(Serialize, Debug, Clone)]
struct Task {
    name: String,
    done: bool,
}

fn main() -> Result<()> {
    let tasks = vec![
        Task {
            name: "write".to_string(),
            done: true,
        },
        Task {
            name: "test".to_string(),
            done: false,
        },
    ];

    let mut pending = 0;
    for t in &tasks {
        if !t.done {
            pending += 1;
        }
        let state = if t.done { "done" } else { "todo" };
        println!("{} - {state}", t.name);
    }
    println!("pending: {pending}");

    let file = temp_dir().join("rustscript-demo.txt");
    fs::write(&file, "hello from script")?;
    let back = fs::read_to_string(&file)?;
    println!("read back: {back}");

    // Windows has no echo binary, it is a cmd builtin, so the command differs
    // per platform even though the output is the same.
    let out = if OS == "windows" {
        Command::new("cmd")
            .arg("/C")
            .arg("echo")
            .arg("from a shell command")
            .output()?
    } else {
        Command::new("echo").arg("from a shell command").output()?
    };
    let text = String::from_utf8_lossy(&out.stdout);
    println!("command said: {}", text.trim());

    let json = serde_json::to_string(&tasks)?;
    println!("json: {json}");

    Ok(())
}
