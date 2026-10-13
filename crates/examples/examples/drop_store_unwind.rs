#!/usr/bin/env rust

// A store makes its value before its place. When the place panics, a tuple or an array
// literal is not built yet, each element is still a temporary of its own, so the elements drop
// newest first. Any other value is whole and drops front to back. Every panic is inside a
// spawned task so the program still exits cleanly.

struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct Row {
    pair: (T, T),
}

fn pair(id: i64) -> (T, T) {
    (T(id), T(id + 1))
}

fn tuple_literal(at: usize) {
    let mut slots: Vec<(T, T)> = Vec::new();
    let local = T(1);
    slots[at] = (T(10), T(11));
    println!("stored next to {}", local.0);
}

fn array_literal(at: usize) {
    let mut slots: Vec<[T; 3]> = Vec::new();
    slots[at] = [T(20), T(21), T(22)];
    println!("stored {}", slots.len());
}

fn nested_literal(at: usize) {
    let mut slots: Vec<(T, (T, T), Vec<T>)> = Vec::new();
    slots[at] = (T(30), (T(31), T(32)), vec![T(33), T(34)]);
    println!("stored {}", slots.len());
}

fn whole_value(at: usize) {
    let mut slots: Vec<(T, T)> = Vec::new();
    slots[at] = pair(40);
    println!("stored {}", slots.len());
}

fn moved_locals(at: usize) {
    let first = T(50);
    let second = T(51);
    let third = T(52);
    let mut slots: Vec<(T, T, T)> = Vec::new();
    slots[at] = (T(53), second, T(54));
    println!("stored next to {} {}", first.0, third.0);
}

fn through_a_reference(at: usize) {
    let mut slots: Vec<(T, T)> = Vec::new();
    *slots.get_mut(at).expect("a slot") = (T(60), T(61));
    println!("stored {}", slots.len());
}

fn whole_through_a_reference(at: usize) {
    let mut slots: Vec<(T, T)> = Vec::new();
    *slots.get_mut(at).expect("a slot") = pair(70);
    println!("stored {}", slots.len());
}

fn into_a_field(at: usize) {
    let mut rows: Vec<Row> = Vec::new();
    rows[at].pair = (T(80), T(81));
    println!("stored {}", rows[at].pair.0.0);
}

fn run(case: usize) {
    match case {
        0 => tuple_literal(5),
        1 => array_literal(5),
        2 => nested_literal(5),
        3 => whole_value(5),
        4 => moved_locals(5),
        5 => through_a_reference(5),
        6 => whole_through_a_reference(5),
        _ => into_a_field(5),
    }
}

#[tokio::main]
async fn main() {
    for case in 0..8 {
        let handle = tokio::spawn(async move {
            run(case);
        });
        let joined = handle.await;
        println!("case {case} panicked: {}", joined.is_err());
    }
    println!("end of main");
}
