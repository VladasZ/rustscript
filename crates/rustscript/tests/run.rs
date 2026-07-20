//! End to end tests. Each writes a script to a temp file and runs it through
//! the real `rustscript` binary, then checks stdout. The `cargo check` gate is
//! skipped here so the interpreter is exercised on its own and stays fast.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_script(src: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("rustscript_test_{}_{}.rs", std::process::id(), id);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, src).unwrap();
    path
}

/// Run a script that is expected to succeed and return its stdout.
fn run(src: &str) -> String {
    let path = temp_script(src);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("run")
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    assert!(
        out.status.success(),
        "script failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run a script that is expected to fail and return its stderr.
fn run_fail(src: &str) -> String {
    let path = temp_script(src);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("run")
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    assert!(!out.status.success(), "script unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn hello_and_arithmetic() {
    let out = run(r#"
fn main() {
    let name = "world";
    let n = 3 + 4 * 2;
    println!("hi {name} {n}");
}
"#);
    assert_eq!(out, "hi world 11\n");
}

#[test]
fn recursion() {
    let out = run(r#"
fn fib(n: u64) -> u64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() {
    println!("{}", fib(10));
}
"#);
    assert_eq!(out, "55\n");
}

#[test]
fn loops_and_mutation() {
    let out = run(r#"
fn main() {
    let mut sum = 0;
    for i in 1..=5 {
        sum += i;
    }
    let mut n = 0;
    while n < 3 {
        n += 1;
    }
    println!("{sum} {n}");
}
"#);
    assert_eq!(out, "15 3\n");
}

#[test]
fn vec_methods() {
    let out = run(r#"
fn main() {
    let mut v = vec![3, 1, 2];
    v.push(4);
    v.sort();
    let doubled_len = v.len() * 2;
    println!("{:?} {} {}", v, v.contains(&3), doubled_len);
}
"#);
    assert_eq!(out, "[1, 2, 3, 4] true 8\n");
}

#[test]
fn hashmap() {
    let out = run(r#"
use std::collections::HashMap;
fn main() {
    let mut m: HashMap<String, i64> = HashMap::new();
    m.insert("a".to_string(), 1);
    m.insert("b".to_string(), 2);
    println!("{} {}", m.len(), m.get("a").unwrap());
}
"#);
    assert_eq!(out, "2 1\n");
}

#[test]
fn structs_enums_match() {
    let out = run(r#"
enum Shape {
    Circle(f64),
    Rect(f64, f64),
}
struct P { x: i64, y: i64 }
impl P {
    fn sum(&self) -> i64 { self.x + self.y }
}
fn area(s: &Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.0 * r * r,
        Shape::Rect(w, h) => w * h,
    }
}
fn main() {
    let p = P { x: 3, y: 4 };
    println!("{}", p.sum());
    println!("{}", area(&Shape::Rect(2.0, 5.0)));
    println!("{}", area(&Shape::Circle(2.0)));
}
"#);
    assert_eq!(out, "7\n10\n12\n");
}

#[test]
fn option_result_and_question_mark() {
    let out = run(r#"
fn parse(s: &str) -> Result<i64, String> {
    match s.parse::<i64>() {
        Ok(n) => Ok(n),
        Err(_) => Err("bad".to_string()),
    }
}
fn doubled(s: &str) -> Result<i64, String> {
    let n = parse(s)?;
    Ok(n * 2)
}
fn main() {
    println!("{}", doubled("21").unwrap());
    let o: Option<i64> = None;
    println!("{}", o.unwrap_or(99));
}
"#);
    assert_eq!(out, "42\n99\n");
}

#[test]
fn option_context_and_namespaced_map_or_else() {
    let out = run(r#"
use anyhow::Context;
mod helper {
    pub fn fallback() -> String { "fallback".to_string() }
}
fn main() {
    let none: Option<String> = None;
    println!("{}", none.map_or_else(helper::fallback, String::from));
    let some = Some("value".to_string());
    println!("{}", some.map_or_else(helper::fallback, String::from));
    let missing: Option<i64> = None;
    println!("{}", missing.context("missing value").unwrap_err());
    let lazy_missing: Option<i64> = None;
    println!("{}", lazy_missing.with_context(|| "lazy missing").unwrap_err());
}
"#);
    assert_eq!(out, "fallback\nvalue\nmissing value\nlazy missing\n");
}

#[test]
fn format_specs() {
    let out = run(r#"
fn main() {
    println!("{:>5}", 7);
    println!("{:.2}", 3.14159);
    println!("{:?}", "hi");
}
"#);
    assert_eq!(out, "    7\n3.14\n\"hi\"\n");
}

#[test]
fn string_methods() {
    let out = run(r#"
fn main() {
    let s = "the cat sat";
    let words: Vec<String> = s.split(" ").collect();
    println!("{} {}", words.len(), s.to_uppercase());
    println!("{}", "  trim  ".trim());
}
"#);
    assert_eq!(out, "3 THE CAT SAT\ntrim\n");
}

#[test]
fn string_rsplit() {
    let out = run(r#"
fn main() {
    let name = "python3Packages.python-lsp-server";
    println!("{}", name.rsplit('.').next().unwrap_or(name));
    let parts: Vec<String> = "a.b.c".rsplit('.').collect();
    println!("{}", parts.join(","));
}
"#);
    assert_eq!(out, "python-lsp-server\nc,b,a\n");
}

#[test]
fn fs_roundtrip() {
    let out = run(r#"
use std::fs;
fn main() -> anyhow::Result<()> {
    let path = "/tmp/rustscript_fs_test.txt";
    fs::write(path, "payload")?;
    let back = fs::read_to_string(path)?;
    println!("{back}");
    Ok(())
}
"#);
    assert_eq!(out, "payload\n");
}

#[test]
fn shell_command() {
    let out = run(r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("echo").arg("hello").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{}", text.trim());
    println!("{}", out.status.success());
    Ok(())
}
"#);
    assert_eq!(out, "hello\ntrue\n");
}

#[test]
fn command_env_remove() {
    let script = if cfg!(windows) {
        r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("cmd")
        .args(["/C", "if defined RUSTSCRIPT_REMOVE_ME (echo present) else (echo absent)"])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}
"#
    } else {
        r#"
use std::process::Command;
fn main() -> anyhow::Result<()> {
    let out = Command::new("sh")
        .args(["-c", "if [ -z \"${RUSTSCRIPT_REMOVE_ME+x}\" ]; then echo absent; else echo present; fi"])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}
"#
    };
    assert_eq!(run(script), "absent\n");
}

#[test]
fn serde_json_roundtrip() {
    let out = run(r#"
use serde::Serialize;
#[derive(Serialize)]
struct Item { id: i64, name: String }
fn main() -> anyhow::Result<()> {
    let item = Item { id: 7, name: "gadget".to_string() };
    let json = serde_json::to_string(&item)?;
    println!("{json}");
    let parsed = serde_json::from_str(&json)?;
    let id = parsed["id"].clone();
    println!("{id:?}");
    Ok(())
}
"#);
    assert_eq!(out, "{\"id\":7,\"name\":\"gadget\"}\n7\n");
}

#[test]
fn typed_serde_deserialize() {
    let out = run(r##"
use serde::Deserialize;
#[derive(Deserialize)]
struct Point { x: i64, y: i64 }
fn main() -> anyhow::Result<()> {
    let p: Point = serde_json::from_str(r#"{"x":3,"y":4}"#)?;
    println!("{} {}", p.x, p.y);
    let list: Vec<i64> = serde_json::from_str("[1,2,3]")?;
    println!("{:?}", list);
    Ok(())
}
"##);
    assert_eq!(out, "3 4\n[1, 2, 3]\n");
}

#[test]
fn serde_rename_on_serialize_and_to_value() {
    let out = run(r##"
use serde::Serialize;
#[derive(Serialize)]
struct StatusLine {
    #[serde(rename = "type")]
    kind: String,
    command: String,
}
fn main() -> anyhow::Result<()> {
    let line = StatusLine { kind: "command".to_string(), command: "bun x.ts".to_string() };
    let flat = serde_json::to_string(&line)?;
    println!("{flat}");
    let mut data = serde_json::from_str::<serde_json::Value>(r#"{"theme":"light"}"#)?;
    data["statusLine"] = serde_json::to_value(line)?;
    let pretty = serde_json::to_string_pretty(&data)?;
    println!("{pretty}");
    Ok(())
}
"##);
    assert!(
        out.contains(r#""type":"command""#),
        "serialize missing rename: {out}"
    );
    assert!(
        out.contains(r#""type": "command""#),
        "to_value missing rename: {out}"
    );
    assert!(!out.contains("kind"), "raw field name leaked: {out}");
}

#[test]
fn read_dir_iteration() {
    let out = run(r#"
use std::fs;
fn main() -> anyhow::Result<()> {
    let base = "/tmp/rustscript_readdir_test";
    fs::create_dir_all(base)?;
    fs::write(&format!("{base}/one.txt"), "a")?;
    fs::write(&format!("{base}/two.txt"), "b")?;
    let mut names = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names.sort();
    println!("{:?}", names);
    Ok(())
}
"#);
    assert_eq!(out, "[\"one.txt\", \"two.txt\"]\n");
}

#[test]
fn path_ancestors() {
    let out = run(r#"
use std::path::Path;
fn main() {
    let mut paths = Vec::new();
    for path in Path::new("one/two").ancestors() {
        paths.push(path.display().to_string());
    }
    println!("{:?}", paths);
}
"#);
    assert_eq!(out, "[\"one/two\", \"one\", \"\"]\n");
}

#[test]
fn named_temp_file_and_os_string_into_path() {
    let out = run(r#"
use std::env;
use std::path::PathBuf;
use tempfile::NamedTempFile;
fn main() {
    let file = NamedTempFile::new().unwrap();
    println!("{}", file.path().is_file());
    unsafe { env::set_var("RUSTSCRIPT_PATH_TEST", "/tmp/rustscript-path") };
    let path: PathBuf = env::var_os("RUSTSCRIPT_PATH_TEST").map(Into::into).unwrap();
    println!("{}", path.display());
    unsafe { env::remove_var("RUSTSCRIPT_PATH_TEST") };
}
"#);
    assert_eq!(out, "true\n/tmp/rustscript-path\n");
}

#[test]
fn char_ascii_digit() {
    let out = run(r#"
fn main() {
    println!("{} {}", '7'.is_ascii_digit(), 'x'.is_ascii_digit());
    println!("{}", "/dev/disk9".replacen("/dev/disk", "/dev/rdisk", 1));
}
"#);
    assert_eq!(out, "true false\n/dev/rdisk9\n");
}

#[test]
fn regex_matching() {
    let out = run(r#"
use regex::Regex;
fn main() -> anyhow::Result<()> {
    let re = Regex::new(r"(\w+)=(\d+)")?;
    let caps = re.captures("port=8080").unwrap();
    println!("{} {}", &caps[1], &caps[2]);
    println!("{}", re.is_match("x=1"));
    let clean = Regex::new(r"\s+")?.replace_all("a  b   c", "-");
    println!("{clean}");
    Ok(())
}
"#);
    assert_eq!(out, "port 8080\ntrue\na-b-c\n");
}

#[test]
fn lazy_iterator_chains() {
    let out = run(r#"
use regex::Regex;

fn main() {
    let lengths: Vec<usize> = "a bb ccc"
        .split_whitespace()
        .map(|word| word.len())
        .filter(|length| *length > 1)
        .collect();
    let checksum: u64 = "abc".bytes().map(|byte| byte as u64).sum();
    let starts: Vec<usize> = Regex::new(r"\d+")
        .unwrap()
        .find_iter("a1 bb22 c333")
        .map(|found| found.start())
        .collect();
    println!("{:?} {checksum} {:?}", lengths, starts);
}
"#);
    assert_eq!(out, "[2, 3] 294 [1, 5, 9]\n");
}

#[test]
fn shebang_is_ignored() {
    let out = run("#!/usr/bin/env rust\nfn main() { println!(\"ok\"); }\n");
    assert_eq!(out, "ok\n");
}

#[test]
fn error_from_main_exits_nonzero() {
    let err = run_fail(
        r#"
fn main() -> anyhow::Result<()> {
    anyhow::bail!("boom");
}
"#,
    );
    assert!(err.contains("boom"), "stderr was: {err}");
}

#[test]
fn panic_exits_nonzero() {
    let err = run_fail(
        r#"
fn main() {
    let v: Vec<i64> = vec![];
    println!("{}", v[0]);
}
"#,
    );
    assert!(!err.is_empty());
}

#[test]
fn let_else_diverges_on_no_match() {
    let out = run(r#"
fn first_word(s: &str) -> String {
    let Some(w) = s.split_whitespace().next() else {
        return "empty".to_string();
    };
    w.to_string()
}
fn main() {
    println!("{}", first_word("hello there"));
    println!("{}", first_word("   "));
}
"#);
    assert_eq!(out, "hello\nempty\n");
}

#[test]
fn let_else_binds_and_continues_on_match() {
    let out = run(r#"
fn main() {
    let pairs = [("a", 1), ("b", 2)];
    for p in &pairs {
        let (name, n) = *p;
        let Some(doubled) = Some(n * 2) else { continue };
        println!("{name}={doubled}");
    }
}
"#);
    assert_eq!(out, "a=2\nb=4\n");
}

#[test]
fn option_or_else_and_or() {
    let out = run(r#"
fn main() {
    let a: Option<i64> = None;
    let b = a.or_else(|| Some(7));
    println!("{}", b.unwrap());
    let c: Option<i64> = Some(3);
    println!("{}", c.or(Some(9)).unwrap());
    let d: Option<i64> = None;
    println!("{}", d.or(Some(9)).unwrap());
}
"#);
    assert_eq!(out, "7\n3\n9\n");
}

#[test]
fn integer_limits() {
    let out = run(r#"
fn main() {
    println!("{}", 5usize.min(usize::MAX));
    println!("{}", u8::MAX);
    println!("{}", i32::MIN);
    println!("{}", 3i64.saturating_sub(10));
    println!("{}", 10u64.is_multiple_of(2));
}
"#);
    assert_eq!(out, "5\n255\n-2147483648\n-7\ntrue\n");
}

#[test]
fn method_path_function_values() {
    // A method reference like `str::trim` or a constructor like `String::from`
    // used as a function value, the form clippy suggests over a closure.
    let out = run(r#"
fn main() {
    let v = vec![" a ", "b "];
    let trimmed: Vec<&str> = v.iter().copied().map(str::trim).collect();
    let owned: Vec<String> = v.iter().map(ToString::to_string).collect();
    let from: Vec<String> = v.iter().copied().map(String::from).collect();
    println!("{trimmed:?}");
    println!("{}", owned.len());
    println!("{}", from.len());
}
"#);
    assert_eq!(out, "[\"a\", \"b\"]\n2\n2\n");
}

#[test]
fn tokio_hello_runs_on_parallel_engine() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    println!("hello from tokio");
}
"#);
    assert_eq!(out, "hello from tokio\n");
}

#[test]
fn tokio_spawn_join_returns_values() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let a = tokio::spawn(async { 2 + 3 });
    let b = tokio::spawn(async { 10 * 4 });
    let (x, y) = tokio::join!(a, b);
    println!("sum={} prod={}", x.unwrap(), y.unwrap());
}
"#);
    assert_eq!(out, "sum=5 prod=40\n");
}

#[test]
fn tokio_parallel_tasks_capture_and_await() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let count: i64 = "5".parse().unwrap();
    let mut handles = Vec::new();
    for i in 0..count {
        handles.push(tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            i
        }));
    }
    let handles: Vec<_> = handles.into_iter().collect();
    let mut total = 0;
    for h in handles {
        total += h.await.unwrap();
    }
    println!("total={total}");
}
"#);
    assert_eq!(out, "total=10\n");
}

#[test]
fn tokio_tasks_can_yield() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let task = tokio::spawn(async {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        42
    });
    println!("{}", task.await.unwrap());
}
"#);
    assert_eq!(out, "42\n");
}

#[test]
fn tokio_parallel_subprocesses() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let a = tokio::spawn(async {
        let o = std::process::Command::new("echo").arg("A").output().unwrap();
        o.status().success()
    });
    let b = tokio::spawn(async {
        let o = std::process::Command::new("echo").arg("B").output().unwrap();
        o.status().success()
    });
    let (x, y) = tokio::join!(a, b);
    println!("{} {}", x.unwrap(), y.unwrap());
}
"#);
    assert_eq!(out, "true true\n");
}

#[test]
fn tokio_command_env_remove() {
    let script = if cfg!(windows) {
        r#"
#[tokio::main]
async fn main() {
    let out = std::process::Command::new("cmd")
        .args(["/C", "if defined RUSTSCRIPT_REMOVE_ME (exit /b 1) else (exit /b 0)"])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()
        .unwrap();
    println!("{}", out.status().success());
}
"#
    } else {
        r#"
#[tokio::main]
async fn main() {
    let out = std::process::Command::new("sh")
        .args(["-c", "test -z \"${RUSTSCRIPT_REMOVE_ME+x}\""])
        .env("RUSTSCRIPT_REMOVE_ME", "present")
        .env_remove("RUSTSCRIPT_REMOVE_ME")
        .output()
        .unwrap();
    println!("{}", out.status().success());
}
"#
    };
    assert_eq!(run(script), "true\n");
}

#[test]
fn tokio_current_thread_flavor_is_rejected() {
    // Only the multi thread runtime is offered, so an explicit current_thread
    // flavor is rejected at load time.
    let err = run_fail(
        r#"
#[tokio::main(flavor = "current_thread")]
async fn main() {}
"#,
    );
    assert!(err.contains("multi_thread"), "stderr was: {err}");
}

#[test]
fn reqwest_bridge_builds_and_errors_gracefully() {
    // No network: a refused local port must surface as an Err through the whole
    // Client, request builder, and send path, not panic.
    let out = run(r#"
fn main() {
    let client = reqwest::blocking::Client::new();
    let r = client
        .get("http://127.0.0.1:9/")
        .header("X-Test", "1")
        .send();
    println!("{}", r.is_err());
}
"#);
    assert_eq!(out, "true\n");
}

#[test]
fn tokio_as_casts() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let n = 5;
    let f = n as f64 / 2.0;
    let back = f as i64;
    let ch = 65 as char;
    println!("{f} {back} {ch}");
}
"#);
    assert_eq!(out, "2.5 2 A\n");
}

#[test]
fn tokio_user_methods_and_associated_fns() {
    let out = run(r#"
struct P { x: i64, y: i64 }
impl P {
    fn new(x: i64, y: i64) -> P { P { x, y } }
    fn sum(&self) -> i64 { self.x + self.y }
}
fn triple(n: i64) -> i64 { n * 3 }
#[tokio::main]
async fn main() {
    let p = P::new(3, 4);
    println!("{} {}", p.sum(), triple(5));
}
"#);
    assert_eq!(out, "7 15\n");
}

#[test]
fn tokio_module_consts() {
    let out = run(r#"
const LIMIT: i64 = 42;
static NAME: &str = "rustscript";
#[tokio::main]
async fn main() {
    println!("{LIMIT} {NAME}");
}
"#);
    assert_eq!(out, "42 rustscript\n");
}

#[test]
fn tokio_async_reqwest_errors_gracefully() {
    // No network: a refused local port must surface as an Err through the async
    // Client, request builder, and `.send().await` path, not panic.
    let out = run(r#"
#[tokio::main]
async fn main() {
    let client = reqwest::Client::new();
    let r = client
        .get("http://127.0.0.1:9/")
        .header("X-Test", "1")
        .send()
        .await;
    println!("{}", r.is_err());
}
"#);
    assert_eq!(out, "true\n");
}

#[test]
fn std_thread_is_rejected() {
    let err = run_fail(
        r#"
use std::thread;
fn main() {
    let h = thread::spawn(|| 1);
    println!("{}", h.join().unwrap());
}
"#,
    );
    assert!(
        err.contains("std::thread is not supported"),
        "stderr was: {err}"
    );
}

#[test]
fn check_reports_a_method_the_interpreter_lacks() {
    // Valid Rust that `cargo check` accepts, but the parallel engine has no
    // `rposition`, so the coverage gate must catch it without running anything.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("script.rs");
    std::fs::write(
        &file,
        r#"
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let v = vec![1, 2, 3];
    println!("{:?}", v.iter().rposition(|x| *x == 2));
}
"#,
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(["check", file.to_str().unwrap()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "check should fail, stderr: {err}");
    assert!(err.contains("rposition"), "stderr was: {err}");
}

#[test]
fn check_stays_quiet_on_a_script_the_interpreter_supports() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("script.rs");
    std::fs::write(
        &file,
        r#"
fn main() {
    let v = vec![1, 2, 3];
    let doubled: Vec<i64> = v.iter().map(|x| x * 2).collect();
    println!("{} {}", doubled.len(), "ab".repeat(2));
}
"#,
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(["check", file.to_str().unwrap()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "check should pass, stderr: {err}");
}
