#!/usr/bin/env rust

// A binding inside a loop starts a new capture cell every iteration, one
// bound outside keeps the one cell.

fn main() {
    for step in 0..3i64 {
        let mut total = step * 10;
        let mut add = || total += 1;
        add();
        println!("let {total}");
    }

    for mut item in [10i64, 20, 30] {
        let mut bump = || item += 1;
        bump();
        println!("for {item}");
    }

    let mut queue = vec![1i64, 2, 3];
    while let Some(mut taken) = queue.pop() {
        let mut raise = || taken += 100;
        raise();
        println!("while let {taken}");
    }

    for step in 0..3i64 {
        match Some(step * 10) {
            Some(mut found) => {
                let mut grow = || found += 1;
                grow();
                println!("match {found}");
            }
            None => println!("match none"),
        }
    }

    // Bound outside the loop.
    let mut running = 0i64;
    for step in 1..4i64 {
        let mut add = || running += step;
        add();
    }
    println!("running {running}");

    // A closure from one iteration read in the next reaches the variable it
    // captured.
    let mut carried: Vec<i64> = Vec::new();
    for step in 1..4i64 {
        let mut kept = step;
        let mut double = || kept *= 2;
        double();
        double();
        carried.push(kept);
    }
    println!("carried {carried:?}");
}
