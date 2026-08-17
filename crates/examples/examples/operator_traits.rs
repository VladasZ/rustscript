#!/usr/bin/env rust

// Operator trait impls drive the operators: `Add`, `AddAssign`, `Neg`, and
// mixed-type `Mul`.

use std::ops::{Add, AddAssign, Mul, Neg};

#[derive(Clone, Copy, Debug)]
struct V2 {
    x: i32,
    y: i32,
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2 {
            x: self.x + o.x,
            y: self.y + o.y,
        }
    }
}

impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl Mul<i32> for V2 {
    type Output = V2;
    fn mul(self, k: i32) -> V2 {
        V2 {
            x: self.x * k,
            y: self.y * k,
        }
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2 {
            x: -self.x,
            y: -self.y,
        }
    }
}

fn main() {
    let a = V2 { x: 1, y: 2 };
    let b = V2 { x: 3, y: 4 };
    println!("{:?}", a + b);
    let mut c = a;
    c += b;
    println!("{a:?} {c:?}");
    println!("{:?}", a * 3);
    println!("{:?}", -b);
}
