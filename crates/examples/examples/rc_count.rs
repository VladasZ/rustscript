#!/usr/bin/env rust

// `Rc::strong_count` must be right even through function calls.

use std::rc::Rc;

fn one_hop(rc: &Rc<i64>) -> usize {
    Rc::strong_count(rc)
}

fn two_hops(rc: &Rc<i64>) -> usize {
    one_hop(rc)
}

fn main() {
    let a = Rc::new(7);
    println!("direct: {}", Rc::strong_count(&a));
    println!("one hop: {}", one_hop(&a));
    println!("two hops: {}", two_hops(&a));

    let b = Rc::clone(&a);
    println!("shared direct: {}", Rc::strong_count(&a));
    println!("shared one hop: {}", one_hop(&a));
    println!("shared two hops: {}", two_hops(&b));

    drop(b);
    println!("after drop: {}", one_hop(&a));
    println!("value: {}", *a);
}
