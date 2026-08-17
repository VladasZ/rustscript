#!/usr/bin/env rust

// Rc, RefCell, and Arc<Mutex> are the types real Rust uses when sharing
// is the point, so their sharing must stay observable while plain values
// copy. `borrow_mut` through one handle must show through every handle.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

fn main() {
    let shared = Rc::new(RefCell::new(vec![1, 2]));
    let other = Rc::clone(&shared);
    other.borrow_mut().push(3);
    println!("{:?}", shared.borrow());
    println!("{}", Rc::strong_count(&shared));

    let counter = Rc::new(RefCell::new(0));
    let handle = Rc::clone(&counter);
    *handle.borrow_mut() += 5;
    println!("{}", counter.borrow());

    let locked = Arc::new(Mutex::new(String::from("a")));
    let twin = Arc::clone(&locked);
    twin.lock().unwrap().push('b');
    println!("{}", locked.lock().unwrap());

    let cell = RefCell::new(7);
    let old = cell.replace(9);
    println!("{old} {}", cell.borrow());

    let taken = RefCell::new(vec![4]);
    let inner = taken.take();
    println!("{inner:?} {:?}", taken.borrow());
}
