#!/usr/bin/env rust

// `push` says what a vec that was made empty holds. So an item read back out of the vec has a
// type, and a closure that moves it out takes it, where a value of no known type is copied.
// The item drops once.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn call<F: FnOnce() -> Option<T>>(make: F) -> Option<T> {
    make()
}

fn main() {
    let mut source = [T(3), T(1), T(2)].into_iter().peekable();
    let mut small = Vec::new();
    while let Some(item) = source.next_if(|item| item.0 != 2) {
        small.push(item);
    }
    println!("pushed {}", small.len());
    let least = small.into_iter().min();
    println!("least {least:?}");
    let chosen = call(|| least);
    println!("chosen {chosen:?}");

    // `insert` fills a vec the same way
    let mut front = Vec::new();
    for id in 4..6 {
        front.insert(0, T(id));
    }
    let popped = front.pop();
    let moved = call(|| popped);
    println!("moved {moved:?}");
    println!("end");
}
