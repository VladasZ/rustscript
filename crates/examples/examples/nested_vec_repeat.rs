#!/usr/bin/env rust

// The types inferred inside a `vec![value; count]` have to survive, including the ones in another
// macro nested in the value.

#[derive(Clone, Debug, Default, PartialEq)]
struct Cell {
    row: i64,
    label: String,
}

fn main() {
    let grid = vec![
        vec![
            Cell {
                row: 1,
                ..Default::default()
            },
            Cell::default()
        ];
        2
    ];
    println!("{grid:?}");
    println!("rows {} cols {}", grid.len(), grid[0].len());

    let counts = vec![vec![0u8; 3]; 2];
    println!("{counts:?}");

    let flags = vec![matches!(grid[0][0].row, 1), false];
    println!("{flags:?}");

    let words = vec![vec![String::from("a"), String::default()]; 2];
    println!("{words:?}");
}
