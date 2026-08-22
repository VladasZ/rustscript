//! Float loops the scalar plans run unboxed. The float to int cast is in the semantics test
//! `scalar_float_casts`, pedantic clippy bans it here.

fn main() {
    // a mandelbrot style escape region
    let mut in_set: u32 = 0;
    let mut py: i32 = 0;
    while py < 40 {
        let y0 = f64::from(py) / 20.0 * 3.0 - 1.5;
        let mut px: i32 = 0;
        while px < 40 {
            let x0 = f64::from(px) / 20.0 * 3.0 - 2.0;
            let mut zx = 0.0;
            let mut zy = 0.0;
            let mut it: u32 = 0;
            while zx * zx + zy * zy <= 4.0 && it < 50 {
                let tmp = zx * zx - zy * zy + x0;
                zy = 2.0 * zx * zy + y0;
                zx = tmp;
                it += 1;
            }
            if it == 50 {
                in_set += 1;
            }
            px += 1;
        }
        py += 1;
    }
    println!("in set {in_set}");

    // a `for` range accumulating a float
    let mut sum = 0.0;
    for step in 0..100_000i32 {
        sum += f64::from(step) * 0.5;
        sum += -0.25;
    }
    println!("sum {sum}");

    // NaN comparisons are false on both sides
    let nan = f64::NAN;
    let mut below: i64 = 0;
    let mut above: i64 = 0;
    let mut ordered: i64 = 0;
    let mut probes: i64 = 0;
    while probes < 100 {
        if nan < 1.0 {
            below += 1;
        }
        if nan > 1.0 {
            above += 1;
        }
        if 1.0 <= nan {
            ordered += 1;
        }
        probes += 1;
    }
    println!("below {below} above {above} ordered {ordered}");

    // division by zero is infinity, and it survives writeback
    let mut harmonic = 0.0;
    let mut denom = 4.0;
    let mut steps: i64 = 0;
    while steps < 5 {
        harmonic += 1.0 / denom;
        harmonic += 5.5 % 2.0;
        denom -= 1.0;
        steps += 1;
    }
    println!("harmonic {harmonic}");

    // float vec elements in while and for plans
    let mut prices = vec![1.0, 2.5, 4.0, 8.5];
    let count = prices.len();
    let mut slot = 0;
    while slot < count {
        prices[slot] = prices[slot] * 1.5 + 0.25;
        slot += 1;
    }
    let mut total = 0.0;
    for price in prices {
        total += price;
    }
    println!("total {total}");
}
