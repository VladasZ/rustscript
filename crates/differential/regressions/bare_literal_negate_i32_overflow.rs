//! Bare literals are `i32`, so `-(i32::MIN)` must panic and not widen to i64. Seed 255775.

fn diff_opaque_true() -> bool {
    true
}

fn main() {
    let value = -(if diff_opaque_true() { -2147483648 } else { 0 });
    println!("{value}");
}
