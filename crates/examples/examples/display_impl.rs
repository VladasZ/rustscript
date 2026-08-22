#!/usr/bin/env rust


use std::fmt;

struct Point {
    x: i32,
    y: i32,
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl fmt::Debug for Point {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "P[{}/{}]", self.x, self.y)
    }
}

enum Shade {
    Dark,
    Light,
}

impl fmt::Display for Shade {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Shade::Dark => f.write_str("dark"),
            Shade::Light => f.write_str("light"),
        }
    }
}

fn main() {
    let p = Point { x: 1, y: 2 };
    println!("{p}");
    println!("{p:?}");
    let rendered = p.to_string();
    println!("{rendered}");
    println!("{} {}", Shade::Dark, Shade::Light);
    let s = format!("at {p} going {}", Shade::Light);
    println!("{s}");
}
