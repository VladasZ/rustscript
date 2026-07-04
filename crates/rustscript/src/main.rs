mod checker;
mod interpreter;

use std::path::Path;

use anyhow::{Result, bail};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("rustscript error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "run" => {
            let file = args.next().ok_or_else(|| err_usage())?;
            run(&file, true)
        }
        "check" => {
            let file = args.next().ok_or_else(|| err_usage())?;
            let source = std::fs::read_to_string(&file)?;
            checker::check(Path::new(&file), &source)?;
            println!("ok");
            Ok(())
        }
        "clean" => checker::clean(),
        "-h" | "--help" | "help" | "" => {
            print_usage();
            Ok(())
        }
        // `rustscript file.rs` and the shebang form both land here.
        path if path.ends_with(".rs") => run(path, true),
        other => bail!("unknown command `{other}`, try `rustscript help`"),
    }
}

fn run(file: &str, check_first: bool) -> Result<()> {
    let path = Path::new(file);
    let source = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {file}: {e}"))?;

    if check_first {
        checker::check(path, &source)?;
    }

    let ast = syn::parse_file(&source)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;
    let interp = interpreter::Interp::load(&ast)?;
    interp.run_main()
}

fn err_usage() -> anyhow::Error {
    anyhow::anyhow!("missing file argument, try `rustscript help`")
}

fn print_usage() {
    println!(
        r"rustscript - run a subset of Rust as a script

usage:
  rustscript run FILE.rs     check then interpret
  rustscript FILE.rs         same as run
  rustscript check FILE.rs   validate with cargo check only
  rustscript clean           clear the check cache
  rustscript help            show this help"
    );
}
