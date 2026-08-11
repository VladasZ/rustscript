#!/usr/bin/env rust

// `Option::take` must empty the place it is called on. It used to answer a
// clone and leave the source untouched, so `while let Some(x) = opt.take()`
// never ended and a taken child stdin stayed Some. Every arm here prints the
// source after the take to lock the emptying in.

struct Slot {
    value: Option<String>,
}

fn main() {
    let mut opt: Option<i64> = Some(5);
    let taken = opt.take();
    println!("local: taken={taken:?} left={opt:?}");
    let again = opt.take();
    println!("empty: taken={again:?} left={opt:?}");

    let mut slot = Slot {
        value: Some("filled".to_string()),
    };
    let value = slot.value.take();
    println!("field: taken={value:?} left={:?}", slot.value);

    let mut cells: Vec<Option<i64>> = vec![Some(1), Some(2), Some(3)];
    let second = cells[1].take();
    println!("index: taken={second:?} left={cells:?}");

    // A computed index is still a place, and its parts must evaluate once.
    let mut offset = 0;
    let mut bump = || {
        offset += 1;
        offset
    };
    let third = cells[bump() + 1].take();
    println!("computed index: taken={third:?} left={cells:?} calls={offset}");

    let mut fuel: Option<i64> = Some(9);
    let mut count = 0;
    while let Some(n) = fuel.take() {
        count += n;
    }
    println!("drained once: {count}");
}
