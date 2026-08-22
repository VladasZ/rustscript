//! Bare literals are `i32`. `-(i32::MIN)` once widened to i64 and printed
//! 2147483648 instead of panicking. From seed 255775.

fn diff_opaque_true() -> bool {
    true
}

fn main() {
    let value = -(if diff_opaque_true() { -2147483648 } else { 0 });
    println!("{value}");
}
