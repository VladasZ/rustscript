#!/usr/bin/env rust

// A closure writes through a `&mut` scalar parameter of the enclosing function. The parameter
// lives in its capture cell, and the caller sees the writes once the function returns.

fn bump(n: &mut i64) {
    let mut inc = || *n += 1;
    inc();
    inc();
}

fn set(flag: &mut bool, count: &mut u32) {
    let mut done = || {
        *flag = true;
        *count *= 3;
    };
    done();
    *count += 1;
}

fn nested(n: &mut i64) {
    let mut outer = || {
        let mut inner = || *n -= 10;
        inner();
        *n *= 2;
    };
    outer();
    println!("inside {}", *n);
}

fn label(name: &mut String) {
    let mut tag = |i: i32| name.push_str(&i.to_string());
    tag(1);
    tag(2);
}

fn main() {
    let mut n = 5;
    bump(&mut n);
    println!("{n}");

    let mut flag = false;
    let mut count = 4u32;
    set(&mut flag, &mut count);
    println!("{flag} {count}");

    let mut m = 30;
    nested(&mut m);
    println!("{m}");

    let mut name = String::from("t");
    label(&mut name);
    println!("{name}");
}
