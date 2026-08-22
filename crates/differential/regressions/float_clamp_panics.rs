// `f64::clamp` must panic when min > max or a bound is NaN.
fn main() {
    println!("{}", 1.5f64.clamp(0.0, 2.0));
    println!("{}", 1.5f64.clamp(f64::INFINITY, 2.0));
}
