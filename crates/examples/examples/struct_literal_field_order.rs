#!/usr/bin/env rust

//! The fields of a struct literal run in the order the literal writes them. The struct still
//! prints and drops in the order it declares them.

#[derive(Debug)]
struct Noisy(i64);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug)]
struct Pair {
    first: Noisy,
    second: Noisy,
    third: u8,
}

enum Shape {
    Rect { width: u8, height: u8 },
}

fn make(tag: &str, id: i64) -> Noisy {
    println!("make {tag}");
    Noisy(id)
}

fn number(tag: &str, value: u8) -> u8 {
    println!("number {tag}");
    value
}

fn main() {
    let pair = Pair {
        third: number("third", 3),
        second: make("second", 2),
        first: make("first", 1),
    };
    println!("{pair:?} {} {} {}", pair.first.0, pair.second.0, pair.third);
    let Shape::Rect { width, height } = Shape::Rect {
        height: number("height", 4),
        width: number("width", 5),
    };
    println!("{width} {height}");
    let rest = Pair {
        second: make("rest second", 6),
        ..Pair {
            first: make("base first", 7),
            second: make("base second", 8),
            third: number("base third", 9),
        }
    };
    println!("{rest:?}");
}
