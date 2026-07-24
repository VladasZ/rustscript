// A bare function item used as a value, not called directly. It can be bound to
// a variable and it can be handed to a higher-order method like `map`. The
// path-qualified forms like `str::trim` and `ToString::to_string` are covered by
// other examples, this one covers the bare-name form.

fn double(x: i64) -> i64 {
    x * 2
}

fn describe(x: i64) -> String {
    format!("v{x}")
}

fn main() {
    // Bound to a variable, then called through the value.
    let f = double;
    println!("{}", f(21));

    // Handed to Option::map and Option::map_or.
    let some: Option<i64> = Some(5);
    let none: Option<i64> = None;
    println!("{}", some.map(double).unwrap_or(0));
    println!("{}", none.map(double).unwrap_or(-1));
    println!("{}", some.map_or(0, double));

    // Handed to an iterator adaptor.
    let nums: Vec<i64> = vec![1, 2, 3].into_iter().map(double).collect();
    println!("{nums:?}");

    // A bare function that returns an owned value, over an iterator.
    let labels: Vec<String> = vec![7, 8].into_iter().map(describe).collect();
    println!("{labels:?}");
}
