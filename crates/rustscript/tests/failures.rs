//! Failure behavior parity. Each corpus case runs twice, compiled by the real
//! rustc and interpreted, and both runs must agree on the exit code and on the
//! failure text. This is the failing-path twin of the equivalence suite, which
//! only covers success output.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_script(src: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("rustscript_fail_{}_{}.rs", std::process::id(), id);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, src).unwrap();
    path
}

fn run_interpreted(path: &PathBuf) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("run")
        .arg(path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript")
}

/// Compile with plain rustc, std only, and run the binary.
fn run_compiled(path: &PathBuf) -> Output {
    let bin = path.with_extension("bin");
    let build = Command::new("rustc")
        .args(["--edition", "2024", "-o"])
        .arg(&bin)
        .arg(path)
        .output()
        .expect("failed to launch rustc");
    assert!(
        build.status.success(),
        "corpus case must compile:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&bin).output().expect("failed to run compiled");
    std::fs::remove_file(&bin).unwrap();
    out
}

/// Both runs must exit with `code` and mention `needle` on stderr.
fn assert_parity(src: &str, code: i32, needle: &str) {
    let path = temp_script(src);
    let compiled = run_compiled(&path);
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();

    assert_eq!(
        compiled.status.code(),
        Some(code),
        "compiled exit code differs"
    );
    assert_eq!(
        interpreted.status.code(),
        Some(code),
        "interpreted exit code differs, stderr:\n{}",
        String::from_utf8_lossy(&interpreted.stderr)
    );
    for out in [&compiled, &interpreted] {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(needle),
            "stderr must mention {needle:?}, was:\n{stderr}"
        );
    }
}

#[test]
fn index_out_of_bounds_panics_alike() {
    assert_parity(
        "fn main() {\n    let v = vec![1, 2, 3];\n    let i = 10;\n    println!(\"{}\", v[i]);\n}\n",
        101,
        "index out of bounds: the len is 3 but the index is 10",
    );
}

#[test]
fn unwrap_on_none_panics_alike() {
    assert_parity(
        "fn main() {\n    let v: Vec<i64> = Vec::new();\n    println!(\"{}\", v.first().unwrap());\n}\n",
        101,
        "called `Option::unwrap()` on a `None` value",
    );
}

#[test]
fn expect_panics_with_the_message_alike() {
    assert_parity(
        "fn main() {\n    let v: Vec<i64> = Vec::new();\n    println!(\"{}\", v.first().expect(\"missing cfg\"));\n}\n",
        101,
        "missing cfg",
    );
}

#[test]
fn panic_macro_panics_alike() {
    assert_parity(
        "fn main() {\n    let x = 7;\n    panic!(\"boom {x}\");\n}\n",
        101,
        "boom 7",
    );
}

#[test]
fn divide_by_zero_panics_alike() {
    // The divisor comes from argc so rustc cannot deny the division at
    // compile time. Run with no arguments, argc is 1, the divisor is 0.
    assert_parity(
        "fn main() {\n    let a = 10;\n    let b = std::env::args().count() as i64 - 1;\n    println!(\"{}\", a / b);\n}\n",
        101,
        "attempt to divide by zero",
    );
}

/// Newer rustc adds a thread id to the header, `thread 'main' (123) panicked
/// at`, so the shared needle stays looser and the interpreter's own header is
/// pinned exactly on top.
#[test]
fn both_print_the_panic_header() {
    let src = "fn main() {\n    let v = vec![1];\n    let i = 5;\n    println!(\"{}\", v[i]);\n}\n";
    assert_parity(src, 101, "panicked at");

    let path = temp_script(src);
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    let stderr = String::from_utf8_lossy(&interpreted.stderr);
    assert!(
        stderr.starts_with("thread 'main' panicked at "),
        "interpreter header was: {stderr}"
    );
}

/// Integer overflow aborts like debug Rust in both engines with the same
/// message. The `keep` helper is not `const`, so the compiler cannot fold the
/// operation and reject it, which leaves the overflow to happen at runtime.
#[test]
fn integer_overflow_panics_like_rust() {
    fn overflow(expr: &str) -> String {
        format!("fn keep(n: i64) -> i64 {{ n }}\nfn main() {{ let _ = {expr}; }}\n")
    }
    assert_parity(
        &overflow("keep(i64::MAX) + keep(1)"),
        101,
        "attempt to add with overflow",
    );
    assert_parity(
        &overflow("keep(i64::MIN) - keep(1)"),
        101,
        "attempt to subtract with overflow",
    );
    assert_parity(
        &overflow("keep(3037000500) * keep(3037000500)"),
        101,
        "attempt to multiply with overflow",
    );
    assert_parity(
        &overflow("keep(i64::MIN) / keep(-1)"),
        101,
        "attempt to divide with overflow",
    );
}

#[test]
fn process_exit_code_passes_through_alike() {
    assert_parity("fn main() {\n    std::process::exit(3);\n}\n", 3, "");
}

/// An `Err` out of main exits 1 in both worlds. The message rendering differs
/// by design: a compiled `Result<(), String>` main debug-prints the payload
/// with quotes, while the interpreter prints the anyhow style `Error: msg`
/// that the dominant `anyhow::Result` mains produce. Both carry the text.
#[test]
fn err_from_main_exits_one_alike() {
    let src = "fn main() -> Result<(), String> {\n    Err(\"boom\".to_string())\n}\n";
    assert_parity(src, 1, "boom");

    let path = temp_script(src);
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&interpreted.stderr),
        "Error: boom\n",
        "interpreter must print the anyhow form exactly"
    );
}
