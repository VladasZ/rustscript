// Negating i8::MIN overflows in debug Rust, and the value only becomes
// i8::MIN through a runtime cast, so the panic is a width-tracking event.

fn diff_opaque(x: i64) -> i64 {
    x
}

fn main() {
    let value = diff_opaque(-128i64) as i8;
    println!("unreachable: {}", -value);
}
