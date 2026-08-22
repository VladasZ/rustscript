use pretty_assertions::assert_eq;

use super::common::run;

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
    // method references as function values, the form clippy suggests
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
fn annotated_let_collects_chars_into_string() {
    // the let annotation must reach `collect`, otherwise head and rest stay char lists
    let out = run(r#"
fn idx(arr: &[String], i: usize) -> &str {
    match arr.get(i) {
        Some(s) => s.as_str(),
        None => "missing",
    }
}

fn main() {
    let chars: Vec<char> = "Token: abc ".chars().collect();
    let head: String = chars[0..6].iter().collect();
    let rest: String = chars[7..11].iter().collect();
    let parts = vec![head, rest];
    let token = idx(&parts, 1).trim().to_string();
    println!("{} [{token}]", parts[0]);
}
"#);
    assert_eq!(out, "Token: [abc]\n");
}

#[test]
fn range_patterns_and_mut_string_args() {
    let out = run(r#"
fn shift(out: &mut String, b: u8) {
    match b {
        b'a'..=b'z' => out.push(char::from(b - 32)),
        b'0'..=b'9' => out.push('#'),
        _ => out.push(char::from(b)),
    }
}

fn main() {
    let mut s = String::new();
    for b in "ab3-Z".bytes() {
        shift(&mut s, b);
    }
    println!("[{s}]");
}
"#);
    assert_eq!(out, "[AB#-Z]\n");
}

#[test]
fn concat_joins_strings_and_flattens_vecs() {
    let out = run(r#"
fn main() {
    let words = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let nested = vec![vec![1, 2], vec![3]];
    let flat = nested.concat();
    println!("{} {} {}", words.concat(), flat.len(), flat[2]);
}
"#);
    assert_eq!(out, "abc 3 3\n");
}

#[test]
fn mut_scrutinee_slice_rest_bindings_anchor() {
    // `rest @ ..` must bind the whole rest, not a single element
    let out = run(r#"
fn main() {
    let mut nums = vec![1, 2, 3, 4, 5];
    match &mut nums {
        [first, mid @ .., last] => {
            *first += 100;
            *last += 200;
            println!("mid {mid:?}");
        }
        _ => {}
    }
    println!("{nums:?}");
}
"#);
    assert_eq!(out, "mid [2, 3, 4]\n[101, 2, 3, 4, 205]\n");
}
