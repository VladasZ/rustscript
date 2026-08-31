#!/usr/bin/env rust

const REBOOT_EXIT: i32 = 3;
const NAME: &str = "quit";
const MARK: char = 'x';
const READY: bool = true;
const MASK: u32 = 1 << 3;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dir {
    Up,
    Down,
}

const START: Dir = Dir::Up;

struct Cfg;

impl Cfg {
    const STEP: i64 = 7;
}

struct Point {
    x: i32,
    y: i32,
}

fn classify(code: Option<i32>) -> &'static str {
    match code {
        Some(0) => "ok",
        Some(REBOOT_EXIT) => "reboot",
        _ => "failed",
    }
}

fn main() {
    const LOCAL: i32 = 42;

    println!("{}", classify(Some(3)));
    println!("{}", classify(Some(0)));
    println!("{}", classify(Some(9)));

    let word = "quit";
    match word {
        NAME => println!("named"),
        _ => println!("other"),
    }

    match 'x' {
        MARK => println!("mark"),
        _ => println!("no mark"),
    }

    match Some(true) {
        Some(READY) => println!("ready"),
        _ => println!("waiting"),
    }

    match 8u32 {
        MASK => println!("mask"),
        _ => println!("no mask"),
    }

    match Dir::Down {
        START => println!("start"),
        other => println!("moved {other:?}"),
    }

    match 7i64 {
        Cfg::STEP => println!("step"),
        _ => println!("no step"),
    }

    match 42 {
        LOCAL => println!("local"),
        _ => println!("no local"),
    }

    let take = |v: i32| match v {
        LOCAL => "closure local",
        REBOOT_EXIT => "closure global",
        _ => "closure other",
    };
    println!("{}", take(42));
    println!("{}", take(3));
    println!("{}", take(1));

    match i32::MAX {
        i32::MAX => println!("max"),
        _ => println!("not max"),
    }

    let pair = (3, 'x');
    match pair {
        (REBOOT_EXIT, MARK) => println!("pair"),
        _ => println!("no pair"),
    }

    let items = vec![3, 42];
    match items.as_slice() {
        [REBOOT_EXIT, LOCAL] => println!("slice"),
        _ => println!("no slice"),
    }

    let point = Point { x: 3, y: 42 };
    match point {
        Point { x: REBOOT_EXIT, y } => println!("struct {y}"),
        _ => println!("no struct"),
    }

    match 42 {
        REBOOT_EXIT | LOCAL => println!("or"),
        _ => println!("no or"),
    }

    if let Some(REBOOT_EXIT) = Some(3) {
        println!("if let");
    }

    println!("{}", matches!(3, REBOOT_EXIT));
    println!("{}", matches!(4, REBOOT_EXIT));

    let mut queue = vec![1, 3, 3];
    while let Some(REBOOT_EXIT) = queue.pop() {
        println!("popped reboot");
    }
    println!("{queue:?}");

    let none: Option<i32> = None;
    match none {
        Some(REBOOT_EXIT) => println!("some"),
        None => println!("none"),
        _ => println!("rest"),
    }
}
