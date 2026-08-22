use pretty_assertions::assert_eq;

use super::common::{embed_path, run, run_fail};

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
    // only the multi thread runtime exists, so `current_thread` is rejected
    let err = run_fail(
        r#"
#[tokio::main(flavor = "current_thread")]
async fn main() {}
"#,
    );
    assert!(err.contains("multi_thread"), "stderr was: {err}");
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
    // same for the async client
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
fn tokio_range_patterns_and_mut_string_args() {
    let out = run(r#"
fn tag(out: &mut String, c: char) {
    match c {
        'a'..='z' => out.push('l'),
        'A'..='Z' => out.push('u'),
        _ => out.push('?'),
    }
}

#[tokio::main]
async fn main() {
    let mut s = String::new();
    for c in "aZ!".chars() {
        tag(&mut s, c);
    }
    println!("[{s}]");
}
"#);
    assert_eq!(out, "[lu?]\n");
}

#[test]
fn tokio_fs_bridge_reads_dirs_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hi there").unwrap();
    let src = format!(
        r#"
#[tokio::main]
async fn main() {{
    let text = std::fs::read_to_string("{file}").unwrap();
    let bytes = std::fs::read("{file}").unwrap();
    let lossy = String::from_utf8_lossy(&bytes).to_string();
    let meta = std::fs::metadata("{file}").unwrap();
    let entries = std::fs::read_dir("{dir}").unwrap();
    for entry in entries {{
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_file = match entry.file_type() {{
            Ok(ft) => ft.is_file(),
            Err(_) => false,
        }};
        println!("{{name}} {{is_file}}");
    }}
    println!("{{text}} {{lossy}} {{}}", meta.is_file());
}}
"#,
        file = embed_path(&file),
        dir = embed_path(dir.path())
    );
    let out = run(&src);
    assert_eq!(out, "a.txt true\nhi there hi there true\n");
}

#[test]
fn tokio_numeric_conversions_and_env_consts() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let oct = i64::from_str_radix("644", 8).unwrap();
    let narrow = u8::try_from(300).is_err();
    let wide = i64::from(42);
    let os = std::env::consts::OS;
    println!("{oct} {narrow} {wide} {}", os.is_empty());
}
"#);
    assert_eq!(out, "420 true 42 false\n");
}

#[test]
fn tokio_slices_strings_and_vecs() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let s = "hello world";
    let v = vec![1, 2, 3, 4, 5];
    let mut tail: Vec<i64> = Vec::new();
    tail.extend_from_slice(&v[2..]);
    println!("{} {} {} {}", &s[6..], &s[0..5], tail.len(), tail[0]);
}
"#);
    assert_eq!(out, "world hello 3 3\n");
}

#[test]
fn tokio_streams_and_home_dir() {
    let out = run(r#"
use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    let tty = std::io::stdout().is_terminal();
    let home = dirs::home_dir().unwrap();
    let sub = home.join("x");
    println!("{tty} {}", sub.display().to_string().len() > 2);
}
"#);
    assert_eq!(out, "false true\n");
}

#[test]
fn tokio_concat_joins_strings() {
    let out = run(r#"
#[tokio::main]
async fn main() {
    let words = vec!["x".to_string(), "y".to_string()];
    println!("{}", words.concat());
}
"#);
    assert_eq!(out, "xy\n");
}

#[test]
fn tokio_mutex_concurrent_compound_assign() {
    // a read then write through the guard as 2 steps loses updates under this load, the fused
    // `DerefBinAssign` op prevents it
    let out = run(r#"
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let shared = Arc::new(Mutex::new(0i64));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let m = Arc::clone(&shared);
        handles.push(tokio::spawn(async move {
            for _ in 0..500 {
                let mut guard = m.lock().await;
                *guard += 1;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    println!("total {}", *shared.lock().await);
}
"#);
    assert_eq!(out, "total 4000\n");
}
