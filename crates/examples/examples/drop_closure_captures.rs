#!/usr/bin/env rust

// A binding a closure writes lives in a capture cell from its `let` on, so a store before the
// closure and a read after it see the same place, a move out of it empties the cell, and the
// scope end drops what the cell holds. A `move` closure that reads only fields that copy leaves
// the value with the frame, and one that takes a value drops it at its own end.

#[derive(Debug, Clone, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn counter_in_loop() {
    let mut v: i8 = 0;
    println!("start {v}");
    for _ in 0..2 {
        v = -30;
        let mut double = || -> i8 {
            v += v;
            v
        };
        println!("doubled {} after {v}", double());
    }
}

fn reassign_after_move() {
    let mut v = D(1);
    let u = v;
    v = D(3);
    let n: i64 = Vec::<u8>::new().into_iter().map(|_| v.0).sum();
    println!("moved {} kept {} n {n}", u.0, v.0);
}

fn writes_through_the_cell() {
    let mut v = D(1);
    let mut set = || v = D(2);
    set();
    println!("set {}", v.0);
    {
        let mut again = || v = D(4);
        again();
    }
    v = D(5);
    println!("assigned {}", v.0);
}

fn move_out_of_the_cell() {
    let mut v = D(1);
    let mut set = || v = D(2);
    set();
    let taken = v;
    println!("taken {}", taken.0);
}

fn mem_and_option_on_captures() {
    let mut v = D(1);
    let mut swap = || {
        let old = std::mem::replace(&mut v, D(2));
        println!("old {}", old.0);
    };
    swap();
    println!("now {}", v.0);
    let mut slot: Option<D> = Some(D(9));
    let mut take = || slot.take();
    let got = take();
    println!("got {} left {}", got.map_or(0, |d| d.0), slot.is_some());
}

fn move_closures() {
    let owned = D(7);
    let show = move || owned.0;
    println!("show {}", show());
    let mut field = D(1);
    let read = move || field.0;
    field = D(5);
    println!("read {} field {}", read(), field.0);
}

fn main() {
    counter_in_loop();
    reassign_after_move();
    writes_through_the_cell();
    move_out_of_the_cell();
    mem_and_option_on_captures();
    move_closures();
}
