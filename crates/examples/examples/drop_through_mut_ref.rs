#!/usr/bin/env rust

// A value moved into `*target` through a `&mut` parameter belongs to the caller afterwards,
// the callee's locals must not drop it.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct S {
    f0: Option<u32>,
}

fn w1(target: &mut S) {
    let cur: S = target.clone();
    let m: HashMap<usize, S> = HashMap::new();
    *target = m.get(&2usize).cloned().unwrap_or(cur);
}

fn w2(target: &mut S) {
    let cur: S = target.clone();
    let m: HashMap<usize, S> = HashMap::new();
    let picked = m.get(&2usize).cloned().unwrap_or(cur);
    *target = picked;
}

fn w3(target: &mut S) {
    let cur: S = target.clone();
    let empty = if cur.f0.is_some() {
        None
    } else {
        Some(S::default())
    };
    *target = empty.unwrap_or(cur);
}

fn w4(target: &mut D) {
    let cur: D = target.clone();
    *target = D(cur.0 + 100);
}

fn main() {
    let mut a = S { f0: Some(1) };
    w1(&mut a);
    println!("w1 {a:?}");
    let mut b = S { f0: Some(2) };
    w2(&mut b);
    println!("w2 {b:?}");
    let mut c = S { f0: Some(3) };
    w3(&mut c);
    println!("w3 {c:?}");
    let mut d = D(4);
    w4(&mut d);
    println!("w4 {d:?}");
    println!("end");
}
