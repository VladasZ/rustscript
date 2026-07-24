// The bare-function-as-value form on the parallel engine that `#[tokio::main]`
// selects. Option::map is not bridged in tokio mode, so this uses the consumers
// that are: binding the function to a variable and calling it, and handing it to
// an iterator adaptor.

fn triple(x: i64) -> i64 {
    x * 3
}

#[tokio::main]
async fn main() {
    let f = triple;
    println!("{}", f(4));

    let scaled: Vec<i64> = vec![1, 2, 3].into_iter().map(triple).collect();
    println!("{scaled:?}");
}
