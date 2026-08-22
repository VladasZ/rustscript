#!/usr/bin/env rust


trait Shape {
    fn area(&self) -> f64;
    fn name(&self) -> String {
        "shape".to_string()
    }
    fn describe(&self) -> String {
        format!("{} with area {}", self.name(), self.area())
    }
}

struct Circle {
    r: f64,
}
struct Square {
    s: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        3.0 * self.r * self.r
    }
}
impl Shape for Square {
    fn area(&self) -> f64 {
        self.s * self.s
    }
    fn name(&self) -> String {
        "square".to_string()
    }
}

fn show<T: Shape>(shape: &T) {
    println!("{}", shape.describe());
}

fn main() {
    let c = Circle { r: 2.0 };
    let s = Square { s: 3.0 };
    println!("{} {}", c.name(), s.name());
    show(&c);
    show(&s);
    let shapes: Vec<Box<dyn Shape>> =
        vec![Box::new(Circle { r: 1.0 }), Box::new(Square { s: 2.0 })];
    for shape in &shapes {
        println!("{}", shape.describe());
    }
}
