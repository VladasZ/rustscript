mod build_info;
mod checker;
mod interpreter;
mod loader;
mod supported;
mod update;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, exit};

use anyhow::{Error, Result, anyhow, bail};
use mimalloc::MiMalloc;

/// The interpreter is allocation bound and mimalloc handles that far better
/// than the system allocator.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// A deadlock is always an interpreter bug. Without this it is a silent hang
/// with nothing to say which locks are held where.
#[cfg(feature = "deadlock-detection")]
fn spawn_deadlock_watchdog() {
    use std::process::abort;
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    use parking_lot::deadlock::check_deadlock;

    spawn(|| {
        loop {
            sleep(Duration::from_secs(1));
            let cycles = check_deadlock();
            if cycles.is_empty() {
                continue;
            }
            eprintln!("deadlock: {} cycle(s) detected", cycles.len());
            for (i, cycle) in cycles.iter().enumerate() {
                for thread in cycle {
                    eprintln!("deadlock cycle {i}, thread {:#?}:", thread.thread_id());
                    eprintln!("{:#?}", thread.backtrace());
                }
            }
            abort();
        }
    });
}

fn main() {
    #[cfg(feature = "deadlock-detection")]
    spawn_deadlock_watchdog();
    if let Err(e) = real_main() {
        // Exit like a compiled panic or a compiled anyhow main, so `$?` and
        // stderr look the same either way.
        if let Some(p) = e.downcast_ref::<interpreter::ScriptPanic>() {
            if p.file.is_empty() {
                eprintln!("thread 'main' panicked:");
            } else {
                eprintln!("thread 'main' panicked at {}:{}:", p.file, p.line);
            }
            eprintln!("{}", p.rendered);
            exit(101);
        }
        if let Some(r) = e.downcast_ref::<interpreter::ErrReturn>() {
            eprintln!("Error: {}", r.0);
            exit(1);
        }
        // A stable prefix so the differential harness can tell gaps from real
        // failures. The wording check lives next to the messages it matches.
        let rendered = format!("{e:#}");
        if rendered.contains("unsupported")
            || rendered.contains("not supported")
            || rendered.contains("not implemented by")
        {
            eprintln!("rust unsupported: {rendered}");
        } else {
            eprintln!("rust error: {rendered}");
        }
        exit(1);
    }
}

fn real_main() -> Result<()> {
    let all: Vec<String> = env::args().skip(1).collect();
    let cmd = all.first().cloned().unwrap_or_default();
    match cmd.as_str() {
        "check" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            let source = fs::read_to_string(file)?;
            let program = loader::load(Path::new(file), &source)?;
            // Gate 1, valid Rust.
            checker::check(Path::new(file), &program.files, &program.crate_deps)?;
            // Gate 2, the interpreter implements everything it calls.
            check_coverage(&program)?;
            println!("ok");
            Ok(())
        }
        "build" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            build_run(file, &all[2..])
        }
        "clean" => checker::clean(),
        "update" => update::update(&all[1..]),
        "supported" => {
            if all.get(1).map(String::as_str) == Some("md") {
                print!("{}", supported::markdown());
            } else {
                supported::print_supported();
            }
            Ok(())
        }
        "-e" => {
            let code = all
                .get(1)
                .ok_or_else(|| anyhow!("missing code after -e, try `rust help`"))?;
            eval(code, &all[2..])
        }
        "-V" | "--version" => {
            println!("{}", build_info::version());
            Ok(())
        }
        "-h" | "--help" | "help" | "" => {
            print_usage();
            Ok(())
        }
        // An extensionless path still runs when it is a real file, for
        // example a launcher symlink.
        path if Path::new(path).extension() == Some(OsStr::new("rs"))
            || Path::new(path).is_file() =>
        {
            run(path, &all[1..])
        }
        other => bail!("unknown command `{other}`, try `rust help`"),
    }
}

/// Compiling rejects unsupported macros and expressions, the coverage walk
/// adds every method call on every branch.
fn check_coverage(program: &loader::Program) -> Result<()> {
    let interp = interpreter::Interp::load(&program.modules, program.tokio_main)?;
    interp.coverage_gate()
}

fn run(file: &str, script_args: &[String]) -> Result<()> {
    // `NAME cmp ...` runs the script compiled. The word is reserved as a
    // script's first argument.
    if script_args.first().is_some_and(|a| a == "cmp") {
        return build_run(file, &script_args[1..]);
    }

    // Module files are found next to the real script, not the symlink.
    let path = Path::new(file)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = fs::read_to_string(&path).map_err(|e| anyhow!("cannot read {file}: {e}"))?;

    let program = loader::load(&path, &source)?;

    // A real binary sees its own path as argv[0].
    let mut args = vec![file.to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    interpreter::run(&program.modules, program.tokio_main)
}

/// `rust -e 'println!("hi")'`. A complete program runs as written, anything
/// else becomes the body of `fn main`. `?` still works there because an
/// `Err` out of `main` propagates regardless of the signature.
fn eval(code: &str, script_args: &[String]) -> Result<()> {
    let source = if is_program(code) {
        code.to_string()
    } else {
        // The wrapper shares the snippet's first line so trace line numbers
        // match. The newline before `}` survives a trailing comment.
        format!("fn main() {{ {code}\n}}\n")
    };
    // No file, so lookups anchor to the working directory.
    let dir = env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let program = loader::load(&dir.join("-e.rs"), &source)?;

    let mut args = vec!["-e".to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    interpreter::run(&program.modules, program.tokio_main)
}

/// `println!("hi");` alone parses as a file of one macro item, so the
/// `fn main` requirement is what keeps plain statements on the wrapped path.
fn is_program(code: &str) -> bool {
    let Ok(ast) = syn::parse_file(code) else {
        return false;
    };
    ast.items
        .iter()
        .any(|item| matches!(item, syn::Item::Fn(f) if f.sig.ident == "main"))
}

/// Never touches the interpreter.
fn build_run(file: &str, script_args: &[String]) -> Result<()> {
    let path = Path::new(file)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = fs::read_to_string(&path).map_err(|e| anyhow!("cannot read {file}: {e}"))?;
    let program = loader::load(&path, &source)?;

    let bin = checker::build(&path, &program.files, &program.crate_deps)?;
    let status = Command::new(&bin)
        .args(script_args)
        .status()
        .map_err(|e| anyhow!("cannot run compiled binary {}: {e}", bin.display()))?;
    exit(status.code().unwrap_or(1));
}

fn err_usage() -> Error {
    anyhow!("missing file argument, try `rust help`")
}

fn print_usage() {
    println!(
        r"rust - run a subset of Rust as a script

usage:
  rust FILE.rs         interpret the script
  rust -e 'CODE'       run a snippet, arguments after CODE go to it
  rust FILE.rs cmp     compile and run, `cmp` first arg is reserved
  rust build FILE.rs   compile to a native binary, cache it, then run
  rust check FILE.rs   validate with cargo check, does not run
  rust supported       list every bridged method per receiver
  rust clean           clear the cache
  rust update [VER]    install a prebuilt release, the newest one by default,
                       --from-source builds it with cargo instead
  rust --version       show version and build information
  rust help            show this help"
    );
}
