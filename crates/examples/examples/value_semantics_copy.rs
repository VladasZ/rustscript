#!/usr/bin/env rust

// `let b = a` copies, mutating `b` must not change `a`, this must print "1 5".

#[derive(Clone, Copy, Debug)]
struct Point {
    x: i32,
    y: i32,
}

fn main() {
    let origin = Point { x: 1, y: 2 };
    let mut moved = origin;
    moved.x = 5;
    println!("{} {}", origin.x, moved.x);

    let pair = (1, 2);
    let mut shifted = pair;
    shifted.0 = 9;
    println!("{pair:?} {shifted:?}");

    let xs = [1, 2, 3];
    let mut ys = xs;
    ys[0] = 9;
    println!("{xs:?} {ys:?}");

    let mut grid = [[0_i32; 2]; 2];
    let row = grid[0];
    grid[0][1] = 7;
    println!("{row:?} {:?}", grid[0]);

    let source = Point { x: 3, y: 4 };
    let copies = [source; 2];
    let mut edited = copies[1];
    edited.y = 40;
    println!("{} {} {}", source.y, copies[1].y, edited.y);
}
