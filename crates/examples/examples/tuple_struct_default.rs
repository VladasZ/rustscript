#!/usr/bin/env rust

// A derived `Default` on a tuple struct, reached by every spelling, and `std::mem::take` putting
// a user enum's default back.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Trace(i64);

#[derive(Debug, Clone, Default)]
struct S {
    f0: Trace,
    f1: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Off,
    On(u8),
}

fn main() {
    let a = Trace::default();
    let b = <Trace as Default>::default();
    let c = <Trace>::default();
    let s = S {
        f1: 3,
        ..Default::default()
    };
    println!("{a:?} {b:?} {c:?} {:?} {}", s.f0, s.f1);
    let mut mode = Mode::On(7);
    let old = std::mem::take(&mut mode);
    println!("{old:?} {mode:?}");
    let mut trace = Trace(9);
    let previous = std::mem::take(&mut trace);
    println!("{previous:?} {trace:?}");
}
