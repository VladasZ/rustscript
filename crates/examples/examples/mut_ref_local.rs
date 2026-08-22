#!/usr/bin/env rust

// `*r += 10` on a borrowed scalar once panicked with "assignment through a
// non-reference value".

fn main() {
    let mut a = 1;
    let r = &mut a;
    *r += 10;
    println!("{a}");

    let mut v = vec![1, 2];
    let rv = &mut v;
    rv.push(3);
    println!("{v:?}");

    let mut pair = (1, "one".to_string());
    let rp = &mut pair;
    rp.0 = 2;
    rp.1.push('!');
    println!("{pair:?}");

    let mut nums = vec![10, 20, 30];
    let first = &mut nums[0];
    *first += 5;
    println!("{nums:?}");

    let mut text = String::from("hi");
    let rt = &mut text;
    rt.push_str(" there");
    println!("{text}");
}
