mod checker;
mod interpreter;
mod loader;

use std::path::Path;

use anyhow::{Result, bail};
use mimalloc::MiMalloc;

/// Value churn makes the interpreter allocation bound, and mimalloc handles
/// that pattern far better than the system allocator.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("rust error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let cmd = all.first().cloned().unwrap_or_default();
    match cmd.as_str() {
        "run" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            run(file, true, &all[2..])
        }
        "check" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            let source = std::fs::read_to_string(file)?;
            let program = loader::load(Path::new(file), &source)?;
            checker::check(Path::new(file), &program.files)?;
            println!("ok");
            Ok(())
        }
        "clean" => checker::clean(),
        "-h" | "--help" | "help" | "" => {
            print_usage();
            Ok(())
        }
        // `rust file.rs` and the shebang form both land here. Everything after
        // the filename is passed through to the script. An extensionless path
        // still runs when it is a real file, e.g. a launcher symlink.
        path if path.ends_with(".rs") || Path::new(path).is_file() => {
            run(path, true, &all[1..])
        }
        other => bail!("unknown command `{other}`, try `rust help`"),
    }
}

fn run(file: &str, check_first: bool, script_args: &[String]) -> Result<()> {
    // A launcher symlink must resolve to the real script so module files are
    // found next to the source, not next to the link.
    let path = Path::new(file).canonicalize().unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {file}: {e}"))?;

    let program = loader::load(&path, &source)?;
    if check_first {
        checker::check(&path, &program.files)?;
    }

    // A real binary sees its own path as argv[0], then the caller's arguments.
    let mut args = vec![file.to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    let interp = interpreter::Interp::load(&program.modules)?;
    interp.run_main()
}

fn err_usage() -> anyhow::Error {
    anyhow::anyhow!("missing file argument, try `rust help`")
}

fn print_usage() {
    println!(
        r"rust - run a subset of Rust as a script

usage:
  rust run FILE.rs     check then interpret
  rust FILE.rs         same as run
  rust check FILE.rs   validate with cargo check only
  rust clean           clear the check cache
  rust help            show this help"
    );
}
