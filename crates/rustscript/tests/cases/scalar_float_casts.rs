// Float casts inside scalar plans. The plan arms `s_cast` and `s_cast_f64`
// must match the compiled program on the saturated, NaN and precision loss
// edges.

fn main() {
    // Saturation past `i32::MAX`.
    let mut grow = 1.0f64;
    let mut caps: i64 = 0;
    let mut casts: i64 = 0;
    while casts < 30 {
        grow *= 3.7;
        caps += (grow as i32) as i64;
        casts += 1;
    }
    println!("caps {caps}");

    // NaN and the infinities, u8 included.
    let specials: [f64; 4] = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 300.9];
    let mut narrow: i64 = 0;
    let mut bytes: i64 = 0;
    for value in specials {
        narrow += (value as i32) as i64;
        bytes += (value as u8) as i64;
    }
    println!("narrow {narrow} bytes {bytes}");

    // Precision loss past 2^53.
    let mut total = 0.0;
    let mut big: i64 = 9_007_199_254_740_993;
    let mut rounds: i64 = 0;
    while rounds < 4 {
        total += (big as f64) - 9_007_199_254_740_992.0;
        big += 1;
        rounds += 1;
    }
    println!("total {total}");
}
