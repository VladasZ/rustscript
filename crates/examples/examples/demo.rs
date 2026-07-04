#!/usr/bin/env rust

use anyhow::Result;
use serde::Serialize;
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

    fs::write("/tmp/rustscript-demo.txt", "hello from script")?;
    let back = fs::read_to_string("/tmp/rustscript-demo.txt")?;
    println!("read back: {back}");

    let out = Command::new("echo").arg("from a shell command").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    println!("command said: {}", text.trim());

    let json = serde_json::to_string(&tasks)?;
    println!("json: {json}");

    Ok(())
}
