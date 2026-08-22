// Bare functions as values under `#[tokio::main]`.

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
