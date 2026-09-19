#!/usr/bin/env rust

// A bare `ref mut` binding over a scalar or a string local borrows the local itself, so a
// write through it lands. An alias made inside a block ends with the block.

fn main() {
    let mut count = 10i64;
    match count {
        ref mut cur if *cur > 5 => *cur *= 2,
        ref mut cur => *cur = 0,
    }
    println!("{count}");

    let mut small = 3i64;
    match small {
        ref mut cur if *cur > 5 => *cur *= 2,
        ref mut cur => *cur = 0,
    }
    println!("{small}");

    let mut text = String::from("ab");
    match text {
        ref mut cur if cur.is_empty() => cur.push('!'),
        ref mut cur => cur.push('c'),
    }
    println!("{text}");

    let mut items = vec![1, 2];
    match items {
        ref mut cur if cur.len() > 1 => cur.push(3),
        ref mut cur => cur.clear(),
    }
    println!("{items:?}");

    let outer = 1;
    let mut total = 5;
    {
        let outer = &mut total;
        *outer += 1;
    }
    println!("{outer} {total}");

    let shadowed = 7;
    let mut target = 0;
    match target {
        ref mut shadowed if *shadowed == 0 => *shadowed = 3,
        _ => {}
    }
    println!("{shadowed} {target}");
}
