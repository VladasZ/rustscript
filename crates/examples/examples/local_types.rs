#!/usr/bin/env rust

//! A `struct`, an `enum`, a `trait` and an `impl` declared inside a function body work like
//! the ones a module declares.

fn area_of(width: u32, height: u32) -> u32 {
    struct Rect {
        width: u32,
        height: u32,
    }

    impl Rect {
        fn area(&self) -> u32 {
            self.width * self.height
        }
    }

    Rect { width, height }.area()
}

fn main() {
    #[derive(Debug, Default, Clone, PartialEq)]
    struct Point {
        x: i32,
        label: String,
        y: u8,
    }

    #[derive(Debug)]
    enum Step {
        Move { dx: i8, dy: i8 },
        Stop,
    }

    trait Describe {
        fn describe(&self) -> String;
    }

    impl Describe for Point {
        fn describe(&self) -> String {
            format!("{} at {} {}", self.label, self.x, self.y)
        }
    }

    type Points = Vec<Point>;

    let point = Point {
        y: 1,
        label: "p".to_string(),
        x: 2,
    };
    let moved = Point {
        label: "q".into(),
        ..point.clone()
    };
    let points: Points = vec![point.clone(), moved, Point::default()];
    println!("{points:?}");
    println!("{} {}", point.describe(), point == points[0]);
    for step in [Step::Move { dy: 1, dx: 2 }, Step::Stop] {
        match step {
            Step::Move { dx, dy } => println!("move {dx} {dy}"),
            Step::Stop => println!("{step:?}"),
        }
    }
    println!("{}", area_of(3, 4));
}
