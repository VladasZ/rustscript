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
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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
fn shebang_is_ignored() {
    let out = run(
        "#!/usr/bin/env rust\nfn main() { println!(\"ok\"); }\n",
    );
    assert_eq!(out, "ok\n");
}

#[test]
fn error_from_main_exits_nonzero() {
    let err = run_fail(r#"
fn main() -> anyhow::Result<()> {
    anyhow::bail!("boom");
}
"#);
    assert!(err.contains("boom"), "stderr was: {err}");
}

#[test]
fn panic_exits_nonzero() {
    let err = run_fail(r#"
fn main() {
    let v: Vec<i64> = vec![];
    println!("{}", v[0]);
}
"#);
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
}
"#);
    assert_eq!(out, "5\n255\n-2147483648\n-7\n");
}
