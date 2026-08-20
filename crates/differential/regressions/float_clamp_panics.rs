// `f64::clamp` panics when min > max or either bound is NaN, with the std
// message. The interpreter once clamped silently.
fn main() {
    println!("{}", 1.5f64.clamp(0.0, 2.0));
    println!("{}", 1.5f64.clamp(f64::INFINITY, 2.0));
}
