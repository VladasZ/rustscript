#!/usr/bin/env rust

use std::{
    fs::{File, create_dir_all, read_dir, write},
    io::Result,
    path::Path,
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const NOT_WATCHED: &[&str] = &["target", "build", "node_modules"];
const POLL: Duration = Duration::from_millis(5);

fn stamp(path: &Path, secs: u64) -> Result<()> {
    File::options()
        .write(true)
        .open(path)?
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
}

/// The count of watched files and the newest change time, like the watch loop of a build script.
fn walk(dir: &Path, state: &mut (usize, SystemTime)) -> Result<()> {
    for entry in read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || NOT_WATCHED.contains(&name.as_ref()) {
            continue;
        }
        let meta = entry.metadata()?;
        if meta.is_dir() {
            walk(&entry.path(), state)?;
            continue;
        }
        state.0 += 1;
        state.1 = state.1.max(meta.modified()?);
    }
    Ok(())
}

fn sources(root: &Path) -> Result<(usize, SystemTime)> {
    let mut state = (0, UNIX_EPOCH);
    walk(root, &mut state)?;
    Ok(state)
}

fn secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn build(round: u32) -> std::result::Result<String, String> {
    if round == 4 {
        return Err(format!("round {round} does not compile"));
    }
    Ok(format!("lib{round}.dylib"))
}

fn list(dir: &Path) -> Result<()> {
    let mut lines = Vec::new();
    for entry in read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let kind = entry.file_type()?;
        lines.push(format!(
            "{:?} dir {} file {} len {} modified {} kind dir {} kind file {} link {}",
            entry.file_name().to_string_lossy(),
            meta.is_dir(),
            meta.is_file(),
            if meta.is_file() { meta.len() } else { 0 },
            meta.modified().is_ok(),
            kind.is_dir(),
            kind.is_file(),
            kind.is_symlink(),
        ));
    }
    lines.sort();
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    create_dir_all(root.join("src/deep"))?;
    create_dir_all(root.join("target/debug"))?;
    create_dir_all(root.join(".git"))?;
    write(root.join("src/main.rs"), "fn main() {}")?;
    write(root.join("src/deep/lib.rs"), "pub fn f() {}\n")?;
    write(root.join("Cargo.toml"), "[package]")?;
    write(root.join("target/debug/out"), "skipped")?;
    write(root.join(".git/HEAD"), "skipped")?;
    write(root.join(".hidden"), "skipped")?;
    stamp(&root.join("src/main.rs"), 1_000)?;
    stamp(&root.join("src/deep/lib.rs"), 3_000)?;
    stamp(&root.join("Cargo.toml"), 2_000)?;
    stamp(&root.join("target/debug/out"), 9_000)?;
    stamp(&root.join(".hidden"), 9_000)?;

    list(root)?;
    list(&root.join("src"))?;

    let older = UNIX_EPOCH + Duration::from_secs(5);
    let newer = UNIX_EPOCH + Duration::from_secs(7);
    println!("{} {}", secs(older.max(newer)), secs(newer.max(older)));
    println!("{} {}", secs(older.min(newer)), secs(older.max(older)));
    println!("{} {} {}", older == newer, older != newer, older < newer);
    println!("{}", older == UNIX_EPOCH + Duration::from_secs(5));
    println!(
        "{}",
        (1usize, older) == (1usize, UNIX_EPOCH + Duration::from_secs(5))
    );
    println!("{}", (1usize, older) != (1usize, newer));
    println!("{}", (2usize, older) == (1usize, older));

    let mut seen = sources(root)?;
    println!("start {} files, newest {}", seen.0, secs(seen.1));
    let mut round = 0;
    let mut builds = 0;
    loop {
        round += 1;
        if round > 6 {
            break;
        }
        sleep(POLL);
        match round {
            2 => stamp(&root.join("src/main.rs"), 4_000)?,
            4 => write(root.join("src/new.rs"), "// new")?,
            5 => stamp(&root.join("target/debug/out"), 9_999)?,
            6 => stamp(&root.join("src/new.rs"), 5_000)?,
            _ => {}
        }
        let now = sources(root)?;
        if now == seen {
            println!("round {round} nothing changed");
            continue;
        }
        let newest_moved = now.1 != seen.1;
        seen = now;
        let started = Instant::now();
        match build(round) {
            Ok(name) => {
                builds += 1;
                let took = started.elapsed().as_secs_f32();
                println!(
                    "round {round} {name} is out, {} files, fast {}",
                    seen.0,
                    took < 5.0
                );
            }
            Err(err) => println!("round {round} the build failed, the old code stays: {err}"),
        }
        println!("round {round} newest moved {newest_moved}");
    }
    println!("{builds} builds, {} files", seen.0);
    Ok(())
}
