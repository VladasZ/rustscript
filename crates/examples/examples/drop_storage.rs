#!/usr/bin/env rust

// Storage shared between handles must survive the drop of one of them. Every case here
// broke once a `Drop` impl was in the program, because a drop takes the storage out of a
// handle it should not own.

use std::collections::HashMap;

struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug, Clone, Default)]
struct Cell {
    f0: u8,
}

fn first_or(map: HashMap<bool, Cell>, fallback: Cell) -> Cell {
    map.get(&true).cloned().unwrap_or(fallback)
}

fn swallow(_: Trace) {
    println!("swallowed");
}

fn main() {
    // a method writeback stores the handle it read, that must not empty the element
    let mut rows = vec![Vec::<usize>::new(); 2];
    rows[0].push(1);
    rows[1].push(2);
    rows[1].push(3);
    println!("rows {rows:?}");
    let mut grid = vec![vec![0usize; 1]; 2];
    grid[0].push(5);
    println!("grid {grid:?}");

    // a copy out of a map temporary lives on after the map dropped
    let mut cells = HashMap::new();
    cells.insert(true, Cell { f0: 127 });
    let cell = first_or(cells, Cell { f0: 1 });
    println!("cell {cell:?} {}", cell.f0);
    let inline = {
        let mut m: HashMap<bool, Cell> = HashMap::new();
        m.insert(true, Cell { f0: 9 });
        m
    }
    .get(&true)
    .cloned()
    .unwrap_or_default();
    println!("inline {inline:?}");

    // `repeat` copies, so a duplicate key drops its own storage only
    let keyed: HashMap<Option<i64>, u16> = [None, Some(7)]
        .repeat(2)
        .into_iter()
        .map(|k| (k, 0u16))
        .collect();
    let mut keys: Vec<Option<i64>> = keyed.keys().copied().collect();
    keys.sort();
    println!("keys {keys:?}");

    // a short circuit terminal leaves its iterator to drop at the semicolon
    let found = vec![Trace(1), Trace(2), Trace(3)]
        .into_iter()
        .any(|t| t.0 == 1);
    println!("found {found}");
    let pos = vec![Trace(4), Trace(5)].into_iter().position(|_| true);
    println!("pos {pos:?}");

    // a fresh match scrutinee drops its unbound part after the statement, not after the arm
    println!(
        "bound {}",
        match (Trace(6), Trace(7)) {
            (first, _) if first.0 > 0 => first.0,
            _ => 0,
        }
    );

    // a `_` parameter owns its argument, a `let _` drops a fresh value at once
    swallow(Trace(8));
    let each = |_: Trace| println!("each");
    vec![Trace(9)].into_iter().for_each(each);
    let _ = Trace(10);
    println!("end");
}
