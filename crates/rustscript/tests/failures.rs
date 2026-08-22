//! Each case runs compiled and interpreted, and both must agree on the exit code and the failure
//! text. The failing path twin of the equivalence suite.

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
        .arg(path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript")
}

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
        "failure case must compile:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&bin).output().expect("failed to run compiled");
    std::fs::remove_file(&bin).unwrap();
    out
}

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
fn char_boundary_slice_panics_alike() {
    assert_parity(
        "fn main() {\n    let s = \"h\u{e9}llo\";\n    println!(\"{}\", &s[0..2]);\n}\n",
        101,
        "end byte index 2 is not a char boundary; it is inside '\u{e9}' (bytes 1..3 of string)",
    );
}

#[test]
fn string_slice_out_of_bounds_panics_alike() {
    assert_parity(
        "fn main() {\n    let s = \"hello\";\n    println!(\"{}\", &s[0..10]);\n}\n",
        101,
        "end byte index 10 is out of bounds for string of length 5",
    );
}

#[test]
fn slice_range_out_of_bounds_panics_alike() {
    assert_parity(
        "fn main() {\n    let v = vec![1, 2, 3];\n    println!(\"{:?}\", &v[1..9]);\n}\n",
        101,
        "range end index 9 out of range for slice of length 3",
    );
}

#[test]
fn inverted_slice_range_panics_alike() {
    assert_parity(
        "fn main() {\n    let v = vec![1, 2, 3];\n    let a = 3;\n    println!(\"{:?}\", &v[a..1]);\n}\n",
        101,
        "slice index starts at 3 but ends at 1",
    );
}

#[test]
fn unwrap_on_err_panics_with_debug_payload_alike() {
    assert_parity(
        "fn main() {\n    let r: Result<i32, String> = Err(\"bad\".to_string());\n    println!(\"{}\", r.unwrap());\n}\n",
        101,
        "called `Result::unwrap()` on an `Err` value: \"bad\"",
    );
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
    // the divisor comes from argc so `rustc` can't reject the division at compile time
    assert_parity(
        "fn main() {\n    let a = 10;\n    let b = std::env::args().count() as i64 - 1;\n    println!(\"{}\", a / b);\n}\n",
        101,
        "attempt to divide by zero",
    );
}

/// The whole stderr of a panic is the same as the compiled one, line, column and note included.
/// Only the thread id in the header differs, it is the id of a different process.
#[test]
fn both_print_the_panic_header() {
    let src = "fn main() {\n    let v = vec![1];\n    let i = 5;\n    println!(\"{}\", v[i]);\n}\n";
    assert_parity(src, 101, "panicked at");

    let path = temp_script(src);
    let compiled = run_compiled(&path);
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    let mask = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let open = text.find(" (").expect("a thread id in the header");
        let close = text[open..].find(") ").expect("a thread id in the header") + open;
        format!("{}(tid{}", &text[..=open], &text[close..])
    };
    assert_eq!(mask(&compiled.stderr), mask(&interpreted.stderr));
}

/// `keep` is not `const`, so the compiler can't fold the overflow and reject it.
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

/// An overflow inside a `while` body panics with the exact message.
#[test]
fn while_overflow_panics_like_rust() {
    let src = "fn main() {\n    let mut n: i64 = 3;\n    let mut rounds: i64 = 0;\n    while n != 0 {\n        n *= 3;\n        rounds += 1;\n    }\n    println!(\"{rounds}\");\n}\n";
    assert_parity(src, 101, "attempt to multiply with overflow");
}

/// An out of bounds store inside a `while` body panics on the exact write.
#[test]
fn while_vec_write_oob_panics_like_rust() {
    let src = "fn main() {\n    let mut v = vec![1i64, 2, 3];\n    let mut i: usize = 0;\n    while i < 5 {\n        v[i] = 0;\n        i += 1;\n    }\n    println!(\"{}\", v[0]);\n}\n";
    assert_parity(
        src,
        101,
        "index out of bounds: the len is 3 but the index is 3",
    );
}

/// An overflow after a map insert panics with the insert already done.
#[test]
fn map_overflow_after_insert_panics_like_rust() {
    let src = "use std::collections::HashMap;\nfn main() {\n    let mut m: HashMap<i64, i64> = HashMap::new();\n    let mut acc: i64 = i64::MAX - 5;\n    for k in 0..10 {\n        m.insert(k, k);\n        acc += 1;\n    }\n    println!(\"{} {acc}\", m.len());\n}\n";
    assert_parity(src, 101, "attempt to add with overflow");
}

/// The read twin of the store test above.
#[test]
fn while_vec_read_oob_panics_like_rust() {
    let src = "fn main() {\n    let v = vec![5i64, 6];\n    let mut sum: i64 = 0;\n    let mut i: usize = 0;\n    while i < 4 {\n        sum += v[i];\n        i += 1;\n    }\n    println!(\"{sum}\");\n}\n";
    assert_parity(
        src,
        101,
        "index out of bounds: the len is 2 but the index is 2",
    );
}

/// An overflow deep in a recursion panics with the exact message.
#[test]
fn recursion_overflow_panics_like_rust() {
    let src = "fn grow(n: i64) -> i64 {\n    if n <= 0 { 1 } else { grow(n - 1) * 3 }\n}\nfn main() {\n    println!(\"{}\", grow(45));\n}\n";
    assert_parity(src, 101, "attempt to multiply with overflow");
}

/// The u64 twin, underflow mid tree.
#[test]
fn recursion_underflow_panics_like_rust() {
    let src = "fn down(n: u64) -> u64 {\n    if n == 0 { 0 } else { down(n - 2) }\n}\nfn main() {\n    println!(\"{}\", down(5));\n}\n";
    assert_parity(src, 101, "attempt to subtract with overflow");
}

#[test]
fn process_exit_code_passes_through_alike() {
    assert_parity("fn main() {\n    std::process::exit(3);\n}\n", 3, "");
}

/// The message is the `Debug` form of the payload, quotes on a `String` included. Not `Display`.
#[test]
fn err_from_main_exits_one_alike() {
    let src = "fn main() -> Result<(), String> {\n    Err(\"boom\".to_string())\n}\n";
    assert_parity(src, 1, "boom");

    let path = temp_script(src);
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&interpreted.stderr),
        "Error: \"boom\"\n",
        "interpreter must debug-print the payload exactly"
    );
}

/// A boxed message stays quoted and an io error prints its `Os { .. }` shape.
#[test]
fn err_from_main_matches_compiled_rendering() {
    assert_parity(
        "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    Err(\"oops\".into())\n}\n",
        1,
        "Error: \"oops\"",
    );
    assert_parity(
        "fn main() -> Result<(), std::io::Error> {\n    let text = std::fs::read_to_string(\"no_such_file_for_this_test\")?;\n    println!(\"{text}\");\n    Ok(())\n}\n",
        1,
        "kind: NotFound",
    );
}

/// Real anyhow `Debug` prints the bare message without quotes. Interpreter only, plain `rustc`
/// has no anyhow.
#[test]
fn err_from_anyhow_main_prints_the_bare_message() {
    for src in [
        "fn main() -> anyhow::Result<()> {\n    anyhow::bail!(\"boom {}\", 42)\n}\n",
        "use anyhow::Result;\n\nfn main() -> Result<()> {\n    anyhow::bail!(\"boom {}\", 42)\n}\n",
    ] {
        let path = temp_script(src);
        let interpreted = run_interpreted(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(interpreted.status.code(), Some(1));
        assert_eq!(
            String::from_utf8_lossy(&interpreted.stderr),
            "Error: boom 42\n",
            "anyhow main must print the bare message"
        );
    }
}

/// The differential harness keys its gap versus bug split on this prefix.
#[test]
fn unsupported_errors_carry_the_stable_prefix() {
    let path =
        temp_script("static mut COUNTER: i64 = 0;\n\nfn main() {\n    println!(\"x\");\n}\n");
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(interpreted.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&interpreted.stderr);
    assert!(
        stderr.starts_with("rust unsupported: "),
        "missing prefix in: {stderr}"
    );
}

/// A runtime gap aborts like a panic but names the gap, so tooling can tell it from a script panic.
#[test]
fn runtime_unsupported_constant_names_the_gap() {
    let path =
        temp_script("fn main() {\n    let x: f64 = f64::LOG2_10;\n    println!(\"{x}\");\n}\n");
    let interpreted = run_interpreted(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(interpreted.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&interpreted.stderr);
    assert!(
        stderr.contains("unsupported constant `f64::LOG2_10`"),
        "missing gap name in: {stderr}"
    );
}

#[test]
fn second_borrow_mut_panics_alike() {
    assert_parity(
        "use std::cell::RefCell;\nfn main() {\n    let cell = RefCell::new(vec![1]);\n    let first = cell.borrow_mut();\n    let second = cell.borrow_mut();\n    println!(\"{} {}\", first.len(), second.len());\n}\n",
        101,
        "RefCell already borrowed",
    );
}

#[test]
fn borrow_mut_during_borrow_panics_alike() {
    assert_parity(
        "use std::cell::RefCell;\nfn main() {\n    let cell = RefCell::new(5);\n    let reader = cell.borrow();\n    *cell.borrow_mut() += 1;\n    println!(\"{reader}\");\n}\n",
        101,
        "RefCell already borrowed",
    );
}

#[test]
fn borrow_during_borrow_mut_panics_alike() {
    assert_parity(
        "use std::cell::RefCell;\nfn main() {\n    let cell = RefCell::new(String::new());\n    let mut writer = cell.borrow_mut();\n    writer.push('a');\n    println!(\"{}\", cell.borrow().len());\n}\n",
        101,
        "RefCell already mutably borrowed",
    );
}

#[test]
fn borrow_inside_same_statement_panics_alike() {
    assert_parity(
        "use std::cell::RefCell;\nfn main() {\n    let cell = RefCell::new(vec![1, 2]);\n    cell.borrow_mut().push(cell.borrow().len() as i32);\n    println!(\"{:?}\", cell.borrow());\n}\n",
        101,
        "RefCell already mutably borrowed",
    );
}
