pub const UNIT: Rect = Rect { w: 1, h: 1 };
pub static SHAPE_SET: &str = "basic";

#[derive(Clone, Debug)]
pub struct Rect {
    pub w: i64,
    pub h: i64,
}

impl Rect {
    pub fn area(&self) -> i64 {
        self.w * self.h
    }
}

#[derive(Clone, Debug)]
pub struct Circle {
    pub r: i64,
}

impl Circle {
    pub fn new(r: i64) -> Circle {
        Circle { r }
    }
    pub fn area(&self) -> i64 {
        3 * self.r * self.r
    }
}

#[derive(Debug)]
pub struct Pair(pub i64, pub i64);

impl Pair {
    pub fn sum(&self) -> i64 {
        self.0 + self.1
    }
}

#[derive(Debug)]
pub struct Origin;
