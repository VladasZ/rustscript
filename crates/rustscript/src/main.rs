mod build_info;
mod checker;
mod interpreter;
mod loader;
mod supported;
mod update;

use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, exit};

use anyhow::{Error, Result, anyhow, bail};
use mimalloc::MiMalloc;

/// Value churn makes the interpreter allocation bound, and mimalloc handles
/// that pattern far better than the system allocator.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    if let Err(e) = real_main() {
        // A script runtime abort reports and exits like a compiled panic,
        // and a `Result::Err` out of main like a compiled anyhow main, so
        // callers checking `$?` or grepping stderr see the same behavior
        // either way the script runs.
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
        eprintln!("rust error: {e:#}");
        exit(1);
    }
}

fn real_main() -> Result<()> {
    let all: Vec<String> = env::args().skip(1).collect();
    let cmd = all.first().cloned().unwrap_or_default();
    match cmd.as_str() {
        "run" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            run(file, &all[2..])
        }
        "check" => {
            let file = all.get(1).ok_or_else(err_usage)?;
            let source = fs::read_to_string(file)?;
            let program = loader::load(Path::new(file), &source)?;
            // Gate one: is it valid Rust. `cargo check` is the authority.
            checker::check(Path::new(file), &program.files, &program.crate_deps)?;
            // Gate two: does this interpreter implement everything it calls.
            // Compiling is enough to reach it, nothing is executed.
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
            // `rust supported md` prints the docs page for regeneration.
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
        // `rust file.rs` and the shebang form both land here. Everything after
        // the filename is passed through to the script. An extensionless path
        // still runs when it is a real file, e.g. a launcher symlink.
        path if path.ends_with(".rs") || Path::new(path).is_file() => run(path, &all[1..]),
        other => bail!("unknown command `{other}`, try `rust help`"),
    }
}

/// Compile the program and report anything the interpreter cannot run.
///
/// Compiling alone already rejects unsupported macros and expressions, so this
/// reaches those too. The coverage walk then adds every method call the VM
/// could make, on every branch, without executing a line.
fn check_coverage(program: &loader::Program) -> Result<()> {
    let engine = if program.tokio_main {
        interpreter::coverage::Engine::Parallel
    } else {
        interpreter::coverage::Engine::Fast
    };
    let interp = interpreter::Interp::load(&program.modules, program.tokio_main)?;
    let findings = interp.coverage(engine);
    if findings.is_empty() {
        return Ok(());
    }
    let mut out = String::new();
    for finding in &findings {
        out.push_str("  ");
        out.push_str(&finding.message());
        out.push('\n');
    }
    let engine_name = if program.tokio_main {
        "the parallel engine, which #[tokio::main] selects"
    } else {
        "the interpreter"
    };
    let (count, verb) = if findings.len() == 1 {
        ("1 method".to_string(), "is")
    } else {
        (format!("{} methods", findings.len()), "are")
    };
    Err(anyhow!(
        "{count} used by this script {verb} not implemented by {engine_name}:\n{}",
        out.trim_end()
    ))
}

fn run(file: &str, script_args: &[String]) -> Result<()> {
    // `NAME cmp ...` runs the script compiled instead of interpreted. Launchers
    // pass the caller's words straight through, so a plain `gh-clone cmp` lands
    // here with `cmp` as the first script argument. That word is reserved, a
    // script must not treat its own first positional argument as `cmp`.
    if script_args.first().is_some_and(|a| a == "cmp") {
        return build_run(file, &script_args[1..]);
    }

    // A launcher symlink must resolve to the real script so module files are
    // found next to the source, not next to the link.
    let path = Path::new(file)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(file).to_path_buf());
    let source = fs::read_to_string(&path).map_err(|e| anyhow!("cannot read {file}: {e}"))?;

    let program = loader::load(&path, &source)?;

    // A real binary sees its own path as argv[0], then the caller's arguments.
    let mut args = vec![file.to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    // `#[tokio::main]` routes to the parallel engine. Everything else runs the
    // single threaded fast engine, unchanged.
    if program.tokio_main {
        return interpreter::run_parallel(&program.modules);
    }

    let interp = interpreter::Interp::load(&program.modules, false)?;
    interp.run_main()
}

/// Run a command line snippet, `rust -e 'println!("hi")'`. A snippet that is
/// already a complete program with its own `fn main` runs as written, so
/// `#[tokio::main]` still selects the parallel engine. Anything else becomes
/// the body of a plain `fn main`. `?` still works there: the interpreter
/// propagates an `Err` out of `main` regardless of the signature, ending the
/// run the same way an `anyhow::Result` main does.
fn eval(code: &str, script_args: &[String]) -> Result<()> {
    let source = if is_program(code) {
        code.to_string()
    } else {
        // The wrapper shares the snippet's first line, so line numbers in
        // error traces match the snippet as typed. The newline before `}`
        // keeps a snippet ending in a comment intact.
        format!("fn main() {{ {code}\n}}\n")
    };
    // The snippet has no file, so module and local crate lookups anchor to the
    // working directory, the same places a script saved there would see.
    let dir = env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let program = loader::load(&dir.join("-e.rs"), &source)?;

    let mut args = vec!["-e".to_string()];
    args.extend(script_args.iter().cloned());
    interpreter::set_script_args(args);

    if program.tokio_main {
        return interpreter::run_parallel(&program.modules);
    }
    let interp = interpreter::Interp::load(&program.modules, false)?;
    interp.run_main()
}

/// Whether the snippet is a complete program: it parses as a file and has a
/// top level `fn main`. A statement list is not, `println!("hi");` alone
/// parses as a file of one macro item, so the main requirement is what keeps
/// plain statements on the wrapped path.
fn is_program(code: &str) -> bool {
    let Ok(ast) = syn::parse_file(code) else {
        return false;
    };
    ast.items
        .iter()
        .any(|item| matches!(item, syn::Item::Fn(f) if f.sig.ident == "main"))
}

/// Compile the script to a native binary, cached by source hash, then run it
/// with the caller's arguments and exit with its status. Unlike `run`, this
/// path never touches the interpreter.
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
  rust run FILE.rs     interpret the script
  rust FILE.rs         same as run
  rust -e 'CODE'       run a snippet, arguments after CODE go to it
  rust FILE.rs cmp     compile and run, `cmp` first arg is reserved
  rust build FILE.rs   compile to a native binary, cache it, then run
  rust check FILE.rs   validate with cargo check, does not run
  rust supported       list every bridged method per receiver and engine
  rust clean           clear the cache
  rust update [VER]    install a prebuilt release, the newest one by default,
                       --from-source builds it with cargo instead
  rust --version       show version and build information
  rust help            show this help"
    );
}
