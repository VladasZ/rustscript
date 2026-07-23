//! Untyped parsing next to float classification and slice writes. The
//! interpreter guesses `"160"` into an int even where the annotation said
//! f64, so the float methods and `f64::EPSILON` comparisons here prove that
//! path behaves exactly like the compiled program.

fn fmt_qty(q: f64) -> String {
    if (q - q.trunc()).abs() < f64::EPSILON {
        return (q.trunc() as i64).to_string();
    }
    format!("{q}")
}

fn main() {
    let whole: f64 = "160".parse().unwrap();
    let frac: f64 = "2.5".parse().unwrap();
    println!("{} {}", fmt_qty(whole), fmt_qty(frac));
    println!("{}", frac.floor());
    println!("{}", (2.25_f64).sqrt());
    println!("{}", "MiXeD".eq_ignore_ascii_case("mixed"));

    let mut buf = vec![0_i64; 6];
    buf[1..4].copy_from_slice(&[7, 8, 9]);
    buf[4..].copy_from_slice(&[1, 2]);
    println!("{buf:?}");
}
