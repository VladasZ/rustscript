#!/usr/bin/env rust

//! `format!` is a block with a statement of its own. A temporary in its arguments drops when
//! the `format!` ends, before the rest of the statement around it runs.

// a `format!` inside the arguments of a print is what this example runs
#![allow(clippy::format_in_format_args)]

struct Loud(i64);

impl Drop for Loud {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make(id: i64) -> Option<Loud> {
    (id > 0).then(|| Loud(id))
}

fn main() {
    println!("a {}", format!("{:?}", make(1).is_some()));
    let picked = match make(2) {
        Some(ref loud) if loud.0 > 5 => 1,
        _ => 2,
    };
    println!("b {}", &format!("{picked}{:?}", make(3).is_none())[..1]);
    println!(
        "c {}",
        format!("{}", make(4).map_or(0, |loud| loud.0)).len()
    );
    let joined = format!("{}-{}", make(5).is_some(), make(6).is_none());
    println!("d {joined}");
    println!(
        "e {}",
        format!("{:?}", make(7).is_some()).len() + format!("{:?}", make(8).is_some()).len()
    );
    let count = format!("{}", make(9).is_some()).len() + {
        println!("f between");
        1
    };
    println!("f {count}");
}
