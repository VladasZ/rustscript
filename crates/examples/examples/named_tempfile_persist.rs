#!/usr/bin/env rust

//! `NamedTempFile::new_in`, `as_file` and `persist`. A settings file is staged next to its target
//! with the same permissions, synced, then renamed over the target in one step.

use std::fs;
use std::io::Write;

use tempfile::NamedTempFile;

fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("settings.json");
    fs::write(&target, "{}\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
    }

    let mut staged = NamedTempFile::new_in(dir.path())?;
    println!(
        "staged in dir: {}",
        staged.path().parent() == Some(dir.path())
    );
    staged
        .as_file()
        .set_permissions(fs::metadata(&target)?.permissions())?;
    staged.write_all(b"{\"hooks\": {}}\n")?;
    staged.as_file().sync_all()?;
    println!("staged len: {}", staged.as_file().metadata()?.len());
    let staged_path = staged.path().to_path_buf();
    let file = staged.persist(&target)?;
    println!("persisted len: {}", file.metadata()?.len());
    println!("staged gone: {}", !staged_path.exists());
    print!("{}", fs::read_to_string(&target)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        println!(
            "mode: {:o}",
            fs::metadata(&target)?.permissions().mode() & 0o777
        );
    }

    let other = NamedTempFile::new_in(dir.path())?;
    let missing = dir.path().join("no/such/dir/file");
    match other.persist(&missing) {
        Ok(_) => println!("unexpected persist"),
        Err(e) => println!("persist failed: {}", !e.to_string().is_empty()),
    }
    println!("left over: {}", fs::read_dir(dir.path())?.count());
    println!("bad dir: {}", NamedTempFile::new_in(&missing).is_err());
    Ok(())
}
