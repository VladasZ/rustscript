#!/usr/bin/env rust

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make(n: i64) -> Vec<T> {
    vec![T(n), T(n + 10)]
}

fn main() {
    // a borrowed temporary lives as long as the loop and drops right after it
    for x in &make(1) {
        println!("a {}", x.0);
    }
    println!("--");
    for x in make(2).iter().rev() {
        println!("b {}", x.0);
    }
    println!("--");
    for (i, x) in make(4).iter().enumerate() {
        println!("c {i} {}", x.0);
    }
    println!("--");
    for x in &make(5) {
        println!("d {}", x.0);
        if x.0 > 0 {
            break;
        }
    }
    println!("--");
    for x in &mut make(6) {
        x.0 += 1;
    }
    println!("--");
    let kept = make(7);
    for (i, x) in kept.iter().enumerate() {
        println!("e {i} {}", x.0);
    }
    for x in &kept {
        println!("f {}", x.0);
    }

    // `take` and `replace` through the `&mut` item of an `iter_mut`
    let mut slots: Vec<Option<i32>> = vec![None, Some(1), Some(2)];
    for slot in &mut slots {
        let old = slot.replace(7);
        println!("{old:?}");
    }
    for slot in slots.iter_mut().skip(1) {
        println!("{:?}", slot.take());
    }
    let mut one = Some(3);
    let handle = &mut one;
    println!("{:?} {:?}", handle.replace(4), handle.take());
    println!("{slots:?} {one:?} end");
}
