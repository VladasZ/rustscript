// A bare function item as a value. The qualified forms like `str::trim` are in other examples.

fn double(x: i64) -> i64 {
    x * 2
}

fn describe(x: i64) -> String {
    format!("v{x}")
}

fn main() {
    // bound to a variable
    let f = double;
    println!("{}", f(21));

    // to `Option::map` and `map_or`
    let some: Option<i64> = Some(5);
    let none: Option<i64> = None;
    println!("{}", some.map_or(0, double));
    println!("{}", none.map_or(-1, double));
    println!("{}", some.map_or(0, double));

    // to an iterator adaptor
    let nums: Vec<i64> = vec![1, 2, 3].into_iter().map(double).collect();
    println!("{nums:?}");

    // returning an owned value
    let labels: Vec<String> = vec![7, 8].into_iter().map(describe).collect();
    println!("{labels:?}");
}
