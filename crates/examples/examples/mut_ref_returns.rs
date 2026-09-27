#!/usr/bin/env rust

// Functions and methods that return `&mut`, a whole parameter or a field or element of one, with
// writes through the result by `=`, `+=`, a method call and a named binding, also when the
// function picks one of several `&mut` parameters at runtime.

struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct Board {
    count: usize,
    items: Vec<i32>,
    trace: Trace,
}

impl Board {
    fn count_mut(&mut self) -> &mut usize {
        &mut self.count
    }

    fn trace_mut(&mut self) -> &mut Trace {
        &mut self.trace
    }

    fn itself(&mut self) -> &mut Board {
        self
    }
}

fn pass(n: &mut usize) -> &mut usize {
    n
}

fn pass_text(s: &mut String) -> &mut String {
    s
}

fn pass_trace(t: &mut Trace) -> &mut Trace {
    t
}

fn count_of(board: &mut Board) -> &mut usize {
    &mut board.count
}

fn items_of(board: &mut Board) -> &mut Vec<i32> {
    &mut board.items
}

fn first(values: &mut [usize]) -> &mut usize {
    &mut values[0]
}

fn pick<'a>(a: &'a mut usize, b: &'a mut usize, first: bool) -> &'a mut usize {
    *a += 1;
    if first { a } else { b }
}

fn longer<'a>(a: &'a mut String, b: &'a mut String) -> &'a mut String {
    if a.len() >= b.len() { a } else { b }
}

fn heavier<'a>(a: &'a mut Trace, b: &'a mut Trace) -> &'a mut Trace {
    if a.0 > b.0 { a } else { b }
}

fn main() {
    let mut n = 1usize;
    *pass(&mut n) += 2;
    *pass(&mut n) *= 3;
    let r = pass(&mut n);
    *r += 1;
    println!("n {n}");
    *pass(&mut n) = 40;
    println!("n {n}");

    let mut text = String::from("a");
    *pass_text(&mut text) += "b";
    pass_text(&mut text).push('c');
    println!("text {text}");

    let mut board = Board {
        count: 0,
        items: Vec::new(),
        trace: Trace(1),
    };
    *board.count_mut() += 3;
    *count_of(&mut board) += 10;
    board.itself().count += 1;
    items_of(&mut board).push(7);
    board.trace_mut().0 = 5;
    let seen = *count_of(&mut board);
    println!(
        "board {} {:?} {} {seen}",
        board.count, board.items, board.trace.0
    );

    let mut values = vec![1usize, 2];
    *first(&mut values) *= 7;
    println!("values {values:?}");

    let mut trace = Trace(7);
    pass_trace(&mut trace).0 += 1;
    *pass_trace(&mut trace) = Trace(20);
    println!("trace {}", trace.0);

    let (mut a, mut b) = (1usize, 2usize);
    *pick(&mut a, &mut b, false) += 5;
    *pick(&mut a, &mut b, true) = 40;
    let chosen = pick(&mut a, &mut b, false);
    *chosen *= 2;
    println!("pick {a} {b}");

    let mut left = String::from("ab");
    let mut right = String::from("c");
    longer(&mut left, &mut right).push('!');
    println!("longer {left} {right}");

    let mut light = Trace(3);
    let mut heavy = Trace(9);
    heavier(&mut light, &mut heavy).0 += 1;
    *heavier(&mut light, &mut heavy) = Trace(30);
    println!("heavier {} {}", light.0, heavy.0);
}
