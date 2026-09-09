#!/usr/bin/env rust

// `ref` and `ref mut` bindings over a place anchor into its storage, so a write through one
// lands in the place. Tuples, structs, elements and a `let` pattern alike.

#[derive(Debug)]
struct S {
    a: i64,
    b: String,
}

fn main() {
    let mut t = (1, 2);
    if let (ref mut x, 2) = t {
        *x += 10;
    }
    println!("{t:?}");

    let mut s = S {
        a: 1,
        b: String::from("x"),
    };
    match s {
        S { ref mut a, ref b } if !b.is_empty() => {
            *a += 1;
        }
        _ => {}
    }
    println!("{s:?}");

    let mut pair = (String::from("p"), 0);
    let (ref mut name, ref count) = pair;
    name.push('q');
    println!("{name} {count}");
    println!("{pair:?}");

    let mut opts = vec![Some(1), None, Some(3)];
    let mut i = 0;
    while let Some(ref mut inner) = opts[i] {
        *inner *= 2;
        i += 1;
    }
    println!("{opts:?}");

    let v = [1, 2, 3];
    if let [ref first, ..] = v[..] {
        println!("first {first}");
    }
}
