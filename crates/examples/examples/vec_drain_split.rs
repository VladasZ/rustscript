#!/usr/bin/env rust

//! `drain` with a range, `split_off`, `dedup_by`, `dedup_by_key`, `rotate_left`,
//! `rotate_right`, `chunks_exact` and `rchunks` on a `Vec` and a `VecDeque`.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, PartialOrd)]
struct Noisy(i64);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn noisy(base: i64) -> Vec<Noisy> {
    (0..4).map(|step| Noisy(base + step)).collect()
}

fn main() {
    let mut items = noisy(70);
    let middle: Vec<Noisy> = items.drain(1..3).collect();
    println!("drain {items:?} {middle:?}");
    items.drain(..1);
    println!("drain unused {items:?}");
    let mut numbers = vec![1, 2, 3, 4, 5, 6];
    let total: i32 = numbers.drain(..=1).sum();
    let tail: Vec<i32> = numbers.drain(2..).collect();
    println!("drain ranges {total} {tail:?} {numbers:?}");

    let mut head = noisy(80);
    let rest = head.split_off(1);
    println!("split_off {head:?} {rest:?}");
    drop(rest);

    let mut keyed = noisy(90);
    keyed.dedup_by_key(|item| item.0 / 2);
    println!("dedup_by_key {keyed:?}");
    let mut paired = noisy(100);
    paired.dedup_by(|later, kept| later.0 - kept.0 == 1 && kept.0 % 2 == 0);
    println!("dedup_by {paired:?}");

    let mut turned = noisy(120);
    turned.rotate_left(1);
    turned.rotate_right(2);
    println!("rotate {turned:?} {}", turned.is_sorted());

    let seven = [1, 2, 3, 4, 5, 6, 7];
    let size = seven.len() / 2;
    println!(
        "chunks {:?} {:?}",
        seven
            .chunks_exact(size)
            .map(<[i32]>::to_vec)
            .collect::<Vec<_>>(),
        seven
            .rchunks(size)
            .map(|chunk| chunk.iter().sum::<i32>())
            .collect::<Vec<_>>()
    );

    let mut deque: VecDeque<Noisy> = noisy(170).into();
    deque.rotate_left(1);
    let back = deque.split_off(2);
    println!("deque {deque:?} {back:?}");
    println!(
        "deque drain {:?}",
        deque.drain(..).map(|item| item.0).collect::<Vec<_>>()
    );
    println!("end");
}
