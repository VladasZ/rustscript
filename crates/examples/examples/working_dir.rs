#!/usr/bin/env rust

// Change the working directory and read it back.

use std::env;
use std::fs;

fn main() -> anyhow::Result<()> {
    let dir = env::temp_dir().join("rustscript_wd");
    fs::create_dir_all(&dir)?;

    env::set_current_dir(&dir)?;
    let cwd = env::current_dir()?;
    let path = cwd.to_string_lossy().to_string();
    println!("now in rustscript_wd: {}", path.ends_with("rustscript_wd"));

    Ok(())
}
