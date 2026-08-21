//! An unsuffixed integer literal that nothing else constrains is `i32`, so
//! arithmetic over nothing but bare literals overflows at `i32`. The campaign
//! found `-(i32::MIN)` widening into an i64 and printing 2147483648 where the
//! real binary panics. From seed 255775.

fn diff_opaque_true() -> bool {
    true
}

fn main() {
    let value = -(if diff_opaque_true() { -2147483648 } else { 0 });
    println!("{value}");
}
