#!/usr/bin/env rust

// Two modules define a function with the same name. Each call must read the signature of the
// function its path resolves to, a table by bare name loses both.

mod counts {
    pub fn keep(n: u32) -> u32 {
        n + 1
    }

    pub fn pick(n: &mut u32) -> u32 {
        *n += 10;
        *n
    }
}

fn keep(items: Vec<String>) -> Vec<String> {
    items
}

fn pick(n: &mut usize) -> &mut usize {
    n
}

fn main() {
    // the parameter type of the callee names the `collect` target
    let names = keep("a b".split(' ').map(|c| format!("<{c}>")).collect());
    println!("{names:?}");

    // the function as a value
    let bumped: Vec<u32> = [1, 2, 3].into_iter().map(counts::keep).collect();
    println!("{bumped:?}");

    // a write through the returned `&mut` parameter lands in the caller's place
    let mut n = 1;
    *pick(&mut n) += 5;
    let mut m = 1;
    let got = counts::pick(&mut m);
    println!("{n} {m} {got}");
}
