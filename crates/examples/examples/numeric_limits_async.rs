// Numeric limit constants on the parallel engine that `#[tokio::main]`
// selects. The width tagged ones like `u16::MAX` used to fall through the
// bridge as an unsupported constant, so this pins every integer width and
// both float precisions there.

#[tokio::main]
async fn main() {
    println!("{} {} {}", u8::MAX, u16::MAX, u32::MAX);
    println!("{} {}", u64::MAX, usize::MAX);
    println!("{} {} {}", i8::MIN, i16::MIN, i32::MIN);
    println!("{} {}", i64::MAX, i64::MIN);
    println!("{} {}", f32::EPSILON, f64::EPSILON);
    println!("{} {}", f32::MIN_POSITIVE, f64::MAX);
    let rows = 40;
    let height = u16::try_from(rows + 4).unwrap_or(u16::MAX);
    println!("height {height}");
}
