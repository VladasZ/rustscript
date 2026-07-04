#!/usr/bin/env rustscript

fn hanoi(n: i64, from: &str, to: &str, via: &str) {
    if n == 0 {
        return;
    }
    hanoi(n - 1, from, via, to);
    println!("move disk {n} from {from} to {to}");
    hanoi(n - 1, via, to, from);
}

fn main() {
    hanoi(3, "A", "C", "B");
}
