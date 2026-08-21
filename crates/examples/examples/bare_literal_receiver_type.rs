#!/usr/bin/env rust

//! An expression built from nothing but unsuffixed integer literals is `i32`,
//! the type Rust gives a literal nothing else constrains. That width picks
//! both the method and the `From` impl an `into()` goes through. `min` and
//! `max` on a number answer in the receiver's own type, unlike the sequence
//! reductions of the same name.

#[derive(Debug)]
enum Wrapped {
    Whole(i32),
    Parsed(std::num::ParseIntError),
}

impl From<i32> for Wrapped {
    fn from(value: i32) -> Self {
        Self::Whole(value)
    }
}

impl From<std::num::ParseIntError> for Wrapped {
    fn from(value: std::num::ParseIntError) -> Self {
        Self::Parsed(value)
    }
}

impl Wrapped {
    fn tag(&self) -> String {
        match self {
            Self::Whole(inner) => format!("whole {inner}"),
            Self::Parsed(inner) => format!("parsed {inner}"),
        }
    }
}

fn opaque_bool(v: bool) -> bool {
    v
}

fn opaque_i8(v: i8) -> i8 {
    v
}

fn main() {
    // The receiver is `i32`, so `into()` picks `From<i32>` and not the other.
    let branched: Wrapped = (if opaque_bool(true) { -1_315_330_440 } else { 0 }).into();
    println!("branched: {branched:?} {}", branched.tag());

    let plain: Wrapped = 7.into();
    println!("plain: {plain:?} {}", plain.tag());

    // `min` on a number keeps the receiver's width, so the default built at
    // the end of the chain is an `i8` and not an empty string.
    let narrowed = Vec::<Option<i8>>::new()
        .into_iter()
        .map(|_| opaque_i8(-1).min(opaque_i8(126)))
        .nth(4)
        .unwrap_or_default();
    println!("narrowed: {narrowed:+2X}");

    // The sequence reductions of the same name still answer an `Option`.
    let reduced = vec![opaque_i8(3), opaque_i8(1)].into_iter().min();
    println!("reduced: {reduced:?}");
}
