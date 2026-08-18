//! Float loops the scalar plans run unboxed: f64 arithmetic and comparisons,
//! float literals, `f64::from` on the loop counter, negation, float vec
//! elements, and the NaN and infinity edges that must match the generic path
//! exactly. The float to int cast, which pedantic clippy bans from compiled
//! examples, is covered by the semantics test `scalar_float_casts`.

fn main() {
    // A mandelbrot style escape region: float literals in the condition and
    // body, an int counter beside them, and `f64::from` feeding the seeds.
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

    // A `for` range accumulating a float through `f64::from`, and a negation
    // that stays a float.
    let mut sum = 0.0;
    for step in 0..100_000i32 {
        sum += f64::from(step) * 0.5;
        sum += -0.25;
    }
    println!("sum {sum}");

    // NaN keeps its partial semantics inside the plan: every ordered
    // comparison answers false, on both sides of the operator.
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

    // Float division by zero makes infinity, never a panic, the remainder
    // runs in the plan too, and the infinite value survives writeback.
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

    // Float elements load and store through the while plan's locked vecs,
    // and a float item source runs through the `for` plan.
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
