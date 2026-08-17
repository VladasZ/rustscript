#!/usr/bin/env rust

// Associated consts on impl blocks resolve as `Type::NAME` values.

struct Limits;

impl Limits {
    const MAX: i32 = 100;
    const LABEL: &'static str = "cap";
}

struct Counter {
    n: i32,
}

impl Counter {
    const STEP: i32 = 5;

    fn bump(&mut self) {
        self.n += Counter::STEP;
    }
}

fn main() {
    println!("{} {}", Limits::MAX, Limits::LABEL);
    let mut c = Counter { n: 0 };
    c.bump();
    c.bump();
    println!("{}", c.n);
    let capped = c.n.min(Limits::MAX);
    println!("{capped}");
}
