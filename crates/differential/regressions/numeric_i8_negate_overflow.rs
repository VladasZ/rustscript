// The value becomes `i8::MIN` only through a runtime cast, so the panic depends on width tracking.

fn diff_opaque(x: i64) -> i64 {
    x
}

fn main() {
    let value = diff_opaque(-128i64) as i8;
    println!("unreachable: {}", -value);
}
