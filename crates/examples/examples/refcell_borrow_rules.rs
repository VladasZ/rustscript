#!/usr/bin/env rust

// `RefCell` borrows are tracked like real Rust. A guard that is still alive blocks the next
// conflicting borrow. A guard that went out of scope, was dropped, or ended with its statement
// does not. `try_borrow_mut` reports the state without panicking.

use std::cell::RefCell;
use std::rc::Rc;

fn describe(cell: &RefCell<Vec<i32>>) -> &'static str {
    match cell.try_borrow_mut() {
        Ok(_) => "free",
        Err(_) => "busy",
    }
}

fn total(cell: &RefCell<Vec<i32>>) -> i32 {
    cell.borrow().iter().sum()
}

fn main() {
    let cell = Rc::new(RefCell::new(vec![1, 2, 3]));
    let shared = Rc::clone(&cell);

    // a temporary guard ends with its statement
    cell.borrow_mut().push(4);
    shared.borrow_mut().push(5);
    println!("{} {}", describe(&cell), total(&cell));

    // a named guard lives to the end of its block
    {
        let guard = cell.borrow();
        println!("{} {}", describe(&cell), guard.len());
    }
    println!("{}", describe(&cell));

    // two shared borrows are fine, a mutable one is not
    let a = cell.borrow();
    let b = cell.borrow();
    println!("{} {} {}", a.len(), b.len(), describe(&cell));
    match cell.try_borrow() {
        Ok(c) => println!("shared ok {}", c.len()),
        Err(e) => println!("{e}"),
    }
    drop(a);
    drop(b);
    println!("{}", describe(&cell));

    // an explicit drop ends the borrow early
    let mut writer = cell.borrow_mut();
    writer.push(6);
    match cell.try_borrow() {
        Ok(_) => println!("free"),
        Err(e) => println!("{e} {e:?}"),
    }
    match cell.try_borrow_mut() {
        Ok(_) => println!("free"),
        Err(e) => println!("{e} {e:?}"),
    }
    drop(writer);
    println!("{} {}", describe(&cell), total(&cell));

    // a guard held in a loop body is released every round
    for i in 0..3 {
        let mut g = cell.borrow_mut();
        g.push(i);
    }
    if cell.borrow().len() > 5 {
        cell.borrow_mut().clear();
    } else {
        cell.borrow_mut().push(1);
    }
    while cell.borrow().len() < 3 {
        cell.borrow_mut().push(0);
    }
    println!("{:?} {}", cell.borrow(), describe(&cell));

    // a `break` or `continue` releases the guards of the scopes it leaves
    for i in 0..3 {
        let g = cell.borrow();
        if i == 1 {
            break;
        }
        println!("round {i} {}", g.len());
    }
    let mut rounds = 0;
    loop {
        let _g = cell.borrow_mut();
        rounds += 1;
        if rounds < 3 {
            continue;
        }
        break;
    }
    'outer: for _ in 0..2 {
        let _a = cell.borrow();
        for j in 0..2 {
            let _b = cell.borrow();
            if j == 0 {
                continue 'outer;
            }
        }
    }
    cell.borrow_mut().push(9);
    println!("{} {}", describe(&cell), total(&cell));

    let counter = RefCell::new(0);
    for _ in 0..4 {
        *counter.borrow_mut() += 1;
    }
    let snapshot = *counter.borrow();
    println!("{snapshot} {}", counter.borrow());

    let cells: Vec<Rc<RefCell<i32>>> = (0..3).map(|i| Rc::new(RefCell::new(i))).collect();
    let sum: i32 = cells.iter().map(|c| *c.borrow()).sum();
    for c in &cells {
        *c.borrow_mut() += 10;
    }
    println!(
        "{sum} {:?}",
        cells.iter().map(|c| *c.borrow()).collect::<Vec<_>>()
    );
}
