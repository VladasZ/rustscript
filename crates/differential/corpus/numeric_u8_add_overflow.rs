// A u8 sum that leaves the width panics in debug Rust. The interpreter
// tracks the real width at runtime, so it panics at the same statement with
// the same message.

fn diff_opaque(x: i64) -> i64 {
    x
}

fn main() {
    let a = diff_opaque(200i64) as u8;
    let b = diff_opaque(100i64) as u8;
    println!("unreachable: {}", a + b);
}
