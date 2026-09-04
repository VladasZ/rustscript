#!/usr/bin/env rust

// A temporary that owns a value drops at the semicolon, a field moved out of a struct or a tuple
// leaves the rest to drop at the owner's end, and a moved out local is never dropped twice.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug, Clone, Default)]
struct S {
    f0: D,
    f1: D,
}

fn main() {
    println!("a: {:?}", Ok::<D, String>(D(3)).ok());
    let present = Some(D(4)).is_some();
    println!("present {present}");
    let items = Vec::from([D(5), D(6)]);
    println!("first {:?} {}", items.first().cloned(), items.len());
    let kept = D(7);
    println!("cl {:?}", kept.clone());
    let front = (D(8), D(9)).0;
    println!("front {front:?}");
    let holder = S {
        f0: D(10),
        f1: D(11),
    };
    let first = holder.f0;
    println!("mid {first:?} {:?}", holder.f1);
    let pair = (D(12), D(13));
    let second = pair.1;
    println!("mid2 {second:?}");
    let mut refilled = S {
        f0: D(14),
        f1: D(15),
    };
    let moved = refilled.f0;
    refilled.f0 = D(16);
    println!("mid3 {moved:?} {refilled:?}");
    let rest = vec![D(17), D(18), D(19)].into_iter().next();
    println!("rest {rest:?}");
    let mut opt = Some(D(20));
    let taken = opt.take();
    println!("taken {taken:?} {opt:?}");
    let mut stack = vec![D(21), D(22)];
    let popped = stack.pop();
    println!("popped {popped:?}");
    let removed = stack.remove(0);
    println!("removed {removed:?} {stack:?}");
    println!("end");
}
