// The amount passes through the opaque helper so the overflow lint can't fold it.

fn diff_opaque(x: i64) -> i64 {
    x
}

fn main() {
    let value = diff_opaque(1i64) as u16;
    let amount = diff_opaque(16i64) as u32;
    println!("unreachable: {}", value << amount);
}
