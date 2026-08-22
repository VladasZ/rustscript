#!/usr/bin/env rust

// `vec![row; n]` gives every row its own storage. A write into one row must not show in the
// others, for nested vectors, arrays and strings alike.

fn main() {
    let mut grid = vec![vec![0; 3]; 2];
    grid[1][2] = 7;
    grid[0][0] += 1;
    println!("{grid:?}");

    let mut bytes = vec![vec![0u8; 2]; 2];
    bytes[0][0] = 1;
    println!("{bytes:?}");

    let row = vec![0; 2];
    let mut copies = vec![row; 3];
    copies[0].push(9);
    copies[2][1] = 5;
    println!("{copies:?} {}", copies[1].len());

    let mut names = vec![String::new(); 2];
    names[0].push('a');
    println!("{names:?}");

    let mut arr = [[0; 2]; 2];
    arr[0][0] = 1;
    println!("{arr:?}");

    let mut deep = vec![vec![vec![0; 1]; 2]; 2];
    deep[1][0][0] = 3;
    println!("{deep:?}");

    let mut rows: Vec<Vec<char>> = vec![Vec::new(); 2];
    for (i, row) in rows.iter_mut().enumerate() {
        row.push(char::from(b'a' + u8::try_from(i).unwrap()));
    }
    println!("{rows:?}");
}
