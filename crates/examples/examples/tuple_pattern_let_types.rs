#!/usr/bin/env rust

//! A `let` with a tuple pattern states each name's type through its own
//! element, so a later `unwrap_or_default()` builds the right default.

fn opaque_i64(v: i64) -> i64 {
    v
}

fn main() {
    let (ratio, count, delta) = (None::<f64>, opaque_i64(7), None::<i8>);
    println!(
        "{:?} {count:?} {:?}",
        ratio.unwrap_or_default(),
        delta.unwrap_or_default()
    );

    let (width, letter) = (None::<u16>, None::<char>);
    println!(
        "{:?} {:?}",
        width.unwrap_or_default(),
        letter.unwrap_or_default()
    );

    // An annotated tuple pattern answers the same way.
    let (level, label): (Option<u8>, Option<String>) = (None, None);
    println!(
        "{:?} {:?}",
        level.unwrap_or_default(),
        label.unwrap_or_default()
    );

    // A payload that is present still wins over the default.
    let (scale, flag) = (Some(2.5f64), None::<bool>);
    println!(
        "{:?} {:?}",
        scale.unwrap_or_default(),
        flag.unwrap_or_default()
    );
}
