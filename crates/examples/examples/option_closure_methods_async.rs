// Option's closure-taking methods on the parallel engine that `#[tokio::main]`
// selects: map, map_or, and_then, filter, unwrap_or_else, is_some_and. The fast
// engine already runs these, this proves the tokio engine matches.

fn triple(x: i64) -> i64 {
    x * 3
}

fn none_i64() -> Option<i64> {
    None
}

#[tokio::main]
async fn main() {
    let s: Option<i64> = Some(4);
    let n = none_i64();

    println!("{}", s.map(triple).unwrap_or(0));
    println!("{}", n.map(triple).unwrap_or(-1));
    println!("{}", s.map_or(100, triple));
    println!("{}", n.map_or(100, triple));
    println!(
        "{}",
        s.and_then(|x| if x > 2 { Some(x + 1) } else { None })
            .unwrap_or(0)
    );
    println!("{}", n.unwrap_or_else(|| triple(3)));
    println!("{:?}", s.filter(|x| *x > 10));
    println!("{}", s.is_some_and(|x| x == 4));
}
