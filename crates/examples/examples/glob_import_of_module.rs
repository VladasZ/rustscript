#!/usr/bin/env rust

//! `use module::*` brings every item of a script module in, and `use super::*` hands a module
//! the items and the imports of its parent. A glob of a glob counts, and 2 modules that glob
//! each other still resolve.

// the glob imports are what this example runs
#![allow(clippy::wildcard_imports)]

use std::collections::BTreeMap;

use shapes::deep::*;
use shapes::*;

const SCALE: u32 = 3;

fn root_helper(n: u32) -> u32 {
    n * SCALE
}

#[derive(Debug, Clone, PartialEq)]
struct Tag(u32);

mod shapes {
    use super::*;

    pub const SIDES: u32 = 4;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Square {
        pub side: u32,
        pub tag: Tag,
    }

    // the default of `tag` and `square` is built from types this module names, the code that
    // asks for it stands in another module
    #[derive(Debug, Default)]
    pub struct Frame {
        pub square: Option<Square>,
        pub corner: Corner,
        pub width: u8,
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    pub struct Corner {
        pub x: i16,
        pub label: String,
    }

    #[derive(Debug)]
    pub enum Kind {
        Small,
        Big(u32),
    }

    impl Square {
        pub fn area(&self) -> u32 {
            root_helper(self.side * self.side)
        }

        pub fn kind(&self) -> Kind {
            if self.side < SIDES {
                Kind::Small
            } else {
                Kind::Big(self.side)
            }
        }
    }

    pub fn build(side: u32) -> Square {
        Square {
            side,
            tag: Tag(side + 1),
        }
    }

    pub fn count(words: &[&str]) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for word in words {
            *out.entry(word.to_string()).or_insert(0) += 1;
        }
        out
    }

    pub mod deep {
        use super::super::*;

        pub fn twice(n: u32) -> u32 {
            root_helper(n) + build(n).side
        }
    }
}

fn main() {
    let square = build(5);
    println!("{square:?} {} {:?}", square.area(), square.kind());
    println!(
        "{:?}",
        Square {
            side: 2,
            tag: Tag(0)
        }
        .kind()
    );
    println!("{SIDES} {}", twice(2));
    println!("{:?}", count(&["a", "b", "a"]));
    let made: Vec<Square> = [1, 2].into_iter().map(build).collect();
    println!("{made:?}");
    let frame = Frame {
        width: 3,
        ..Default::default()
    };
    println!("{frame:?} {} {}", frame.width, frame.square.is_none());
    println!("{:?}", shapes::Frame::default().corner);
    match build(9).kind() {
        Kind::Big(n) => println!("big {n}"),
        Kind::Small => println!("small"),
    }
    {
        use inner_only::*;
        println!("{}", hidden(4));
    }
}

mod inner_only {
    pub fn hidden(n: u32) -> u32 {
        n + 100
    }
}
