#!/usr/bin/env rust


struct Counter {
    n: u32,
}
impl Counter {
    fn bump(&mut self) -> u32 {
        fn step(x: u32) -> u32 {
            x + 1
        }
        self.n = step(self.n);
        self.n
    }
}
fn helper() -> &'static str {
    "module helper"
}
fn outer() -> i64 {
    fn fact(n: i64) -> i64 {
        if n <= 1 { 1 } else { n * fact(n - 1) }
    }
    fn is_even(n: u32) -> bool {
        if n == 0 { true } else { is_odd(n - 1) }
    }
    fn is_odd(n: u32) -> bool {
        if n == 0 { false } else { is_even(n - 1) }
    }
    println!("{} {}", is_even(10), is_odd(7));
    fact(10)
}
fn other() -> String {
    fn helper() -> &'static str {
        "inner helper"
    }
    let v: Vec<String> = [1, 2]
        .iter()
        .map(|x| {
            fn twice(y: i32) -> i32 {
                y * 2
            }
            format!("{}", twice(*x))
        })
        .collect();
    format!("{} {}", helper(), v.join(","))
}
fn generic() -> String {
    fn show<T: std::fmt::Debug>(t: T) -> String {
        format!("<{t:?}>")
    }
    let squares: Vec<i32> = (1..4).map(sq).collect();
    #[allow(clippy::items_after_statements)]
    fn sq(x: i32) -> i32 {
        x * x
    }
    format!("{} {} {squares:?}", show(1), show("a"))
}
fn main() {
    println!("{}", outer());
    println!("{}", other());
    println!("{}", helper());
    println!("{}", generic());
    let mut c = Counter { n: 0 };
    c.bump();
    println!("{}", c.bump());
    {
        fn helper() -> i32 {
            42
        }
        println!("{}", helper());
    }
    println!("{}", helper());
}
