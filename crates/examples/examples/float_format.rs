// Float formatting and comparison semantics. Display prints the shortest
// round-trip decimal with no exponent, Debug keeps a `.0` on whole numbers
// and switches to exponent form outside 1e-4..1e16, and NaN makes every
// ordered comparison false. The values cover negative zero, bankers rounding
// bait, the first integer f64 cannot represent, extremes, subnormals, and
// the special constants.

fn main() {
    let values: [f64; 16] = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        2.5,
        0.1,
        0.30000000000000004,
        1e300,
        1e-300,
        9007199254740993.0,
        1.7976931348623157e308,
        5e-324,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for value in values {
        println!("{value} | {value:?}");
    }

    let constants = [f64::EPSILON, f64::MIN_POSITIVE, f64::MAX, f64::MIN];
    for constant in constants {
        println!("{constant:?}");
    }

    let nan = f64::NAN;
    let same_nan = f64::NAN;
    println!(
        "{} {} {} {}",
        nan < 1.0,
        nan > 1.0,
        nan <= same_nan,
        nan >= same_nan
    );
    println!("{} {}", nan == same_nan, nan != same_nan);
    println!("{} {}", f64::INFINITY > 1e308, -1.0 < f64::INFINITY);

    let mixed = 1.5 + 2.0;
    let zero = 0.0f64;
    let numerator = 0.0f64;
    let quotient = 1.0 / zero;
    let negative_quotient = -1.0 / zero;
    let undefined = numerator / zero;
    println!("{mixed} {quotient} {negative_quotient} {undefined}");
    println!("{:?}", vec![1.0, 2.5, -0.0]);
}
