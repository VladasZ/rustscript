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

/// The interpreter is allocation bound, mimalloc is much faster than the system allocator here.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// A deadlock is always an interpreter bug. Without this it is just a silent hang.
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

/// A Rust panic here is never the script's fault. A script panic is a `ScriptPanic` error that
/// never reaches this hook, and a task panic re-raises with `resume_unwind`, which skips it. The
/// default hook prints an interpreter source path, which reads as if the script had crashed.
fn install_bug_hook() {
    use std::backtrace::{Backtrace, BacktraceStatus};
    use std::panic::{PanicHookInfo, set_hook};
    use std::thread::current;

    set_hook(Box::new(|info: &PanicHookInfo<'_>| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let thread = current();
        let name = thread.name().unwrap_or("<unnamed>");
        let location = info
            .location()
            .map_or_else(|| "unknown location".to_string(), ToString::to_string);
        eprintln!("rust: internal error, this is a bug in the interpreter and not in the script");
        // Same header shape as the default hook. The differential harness reads the reason from
        // the line after `panicked at`, and the exit code stays 101 so it still counts as a panic.
        eprintln!("thread '{name}' panicked at {location}:\n{message}");
        let backtrace = Backtrace::capture();
        if backtrace.status() == BacktraceStatus::Captured {
            eprintln!("{backtrace}");
        } else {
            eprintln!("rust: set RUST_BACKTRACE=1 to see where it came from");
        }
        eprintln!(
            "rust: please report it at https://github.com/VladasZ/rustscript/issues with the script that hit it"
        );
    }));
}

fn main() {
    install_bug_hook();
    #[cfg(feature = "deadlock-detection")]
    spawn_deadlock_watchdog();
    if let Err(e) = real_main() {
        // Exit like a compiled panic or a compiled anyhow main would, same `$?` and same stderr.
        if let Some(p) = e.downcast_ref::<interpreter::ScriptPanic>() {
            eprint!("{}", p.header("main"));
            exit(101);
        }
        if let Some(r) = e.downcast_ref::<interpreter::ErrReturn>() {
            eprintln!("Error: {}", r.0);
            exit(1);
        }
        // Stable prefix so the differential harness can tell a gap from a real failure.
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
            let program = loader::load(Path::new(file), &source, file)?;
            // gate 1, valid Rust
            checker::check(Path::new(file), &program.files, &program.crate_deps)?;
            // gate 2, the interpreter has everything the script calls
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
        // A path without extension still runs if it is a real file, for example a launcher symlink.
        path if Path::new(path).extension() == Some(OsStr::new("rs"))
            || Path::new(path).is_file() =>
        {
            run(path, &all[1..])
        }
        other => bail!("unknown command `{other}`, try `rust help`"),
    }
}

/// Compiling rejects unsupported macros and expressions, the coverage walk checks every method
/// call on every branch.
fn check_coverage(program: &loader::Program) -> Result<()> {
    let interp = interpreter::Interp::load(&program.modules, program.tokio_main)?;
    interp.coverage_gate()
}

fn run(file: &str, script_args: &[String]) -> Result<()> {
    // `NAME cmp ...` runs the script compiled. So `cmp` is reserved as a first argument.
    if script_args.first().is_some_and(|a| a == "cmp") {
        return build_run(file, &script_args[1..]);
    }

    // module files live next to the real script, not the symlink
    let path = Path::new(file)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = fs::read_to_string(&path).map_err(|e| anyhow!("cannot read {file}: {e}"))?;

    let program = loader::load(&path, &source, file)?;

    // a real binary sees its own path as argv[0]
    let mut args = vec![file.to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    interpreter::run(&program.modules, program.tokio_main)
}

/// `rust -e 'println!("hi")'`. A complete program runs as is, anything else becomes the body of
/// `fn main`.
/// `?` still works there because an `Err` out of `main` propagates regardless of the signature.
fn eval(code: &str, script_args: &[String]) -> Result<()> {
    let source = if is_program(code) {
        code.to_string()
    } else {
        // Same first line as the snippet so trace line numbers match. The newline before `}`
        // survives a trailing comment.
        format!("fn main() {{ {code}\n}}\n")
    };
    // no file, so lookups use the working directory
    let dir = env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let program = loader::load(&dir.join("-e.rs"), &source, "-e.rs")?;

    let mut args = vec!["-e".to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    interpreter::run(&program.modules, program.tokio_main)
}

/// `println!("hi");` alone parses as a file with 1 macro item, so we require `fn main` to treat
/// it as a program.
fn is_program(code: &str) -> bool {
    let Ok(ast) = syn::parse_file(code) else {
        return false;
    };
    ast.items
        .iter()
        .any(|item| matches!(item, syn::Item::Fn(f) if f.sig.ident == "main"))
}

fn build_run(file: &str, script_args: &[String]) -> Result<()> {
    let path = Path::new(file)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = fs::read_to_string(&path).map_err(|e| anyhow!("cannot read {file}: {e}"))?;
    let program = loader::load(&path, &source, file)?;

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
