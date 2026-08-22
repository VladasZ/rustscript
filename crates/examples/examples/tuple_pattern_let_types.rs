#!/usr/bin/env rust

//! A tuple pattern `let` states the type of each name through its own element.

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

    // annotated
    let (level, label): (Option<u8>, Option<String>) = (None, None);
    println!(
        "{:?} {:?}",
        level.unwrap_or_default(),
        label.unwrap_or_default()
    );

    let (scale, flag) = (Some(2.5f64), None::<bool>);
    println!(
        "{:?} {:?}",
        scale.unwrap_or_default(),
        flag.unwrap_or_default()
    );
}
