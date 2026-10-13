#!/usr/bin/env rust

// A reference owns nothing, in whatever form it is written. A cast to a slice, a branch that
// hands a reference out, an accessor like `first` and a binding that holds a reference all go
// into a by value generic parameter, and the callee drops nothing. A temporary behind a slice
// lives as long as real Rust keeps it.

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn pick<X>(first: X, second: X, take_first: bool) -> X {
    if take_first { first } else { second }
}

fn make(id: i64) -> Vec<T> {
    vec![T(id), T(id + 1)]
}

// both types hold a reference below their top, so the end of the function drops nothing
fn describe(item: Option<&T>, pair: (&T, &[T])) -> String {
    format!("{item:?} {:?} {}", pair.0, pair.1.len())
}

fn forward(left: &[T], right: &[T], take_left: bool) -> usize {
    let chosen = if take_left { left } else { right };
    {
        let again = chosen;
        println!("again {again:?}");
    }
    println!("picked {:?}", pick(left, right, take_left));
    chosen.len()
}

fn main() {
    let kept = make(1);

    // a cast to a slice and a slice of a temporary, which drops whole at the semicolon
    println!("a {:?}", pick(&[] as &[T], &make(10)[..], false));
    println!("b {:?}", pick(&[] as &[T], &make(20)[1..], false));

    // a branch that hands a reference out
    println!(
        "c {:?}",
        if kept.len() > 5 {
            &[] as &[T]
        } else {
            &kept[..]
        }
    );
    let branch = pick(
        if kept.is_empty() {
            &[] as &[T]
        } else {
            &kept[..1]
        },
        &kept[..],
        true,
    );
    println!("d {branch:?}");

    // an accessor
    println!("e {:?}", pick(kept.first(), kept.last(), false));
    println!("f {}", describe(kept.first(), (&kept[1], &kept[..])));

    // a binding that holds a reference
    let slice = kept.as_slice();
    let whole = &kept;
    let other = make(30);
    println!("g {:?}", pick(slice, &kept[..1], true));
    println!("h {:?}", if kept.len() > 5 { whole } else { &other });
    println!("i {:?}", pick(whole, &other, true));
    println!("j {}", forward(&kept, &other, false));

    // a chain that only lends its items collects references, and those own no item
    {
        let late = kept.iter().filter(|item| item.0 > 1).collect::<Vec<_>>();
        println!("l {late:?}");
        for item in late {
            println!("m {item:?}");
        }
    }
    println!("n {:?}", kept.iter().rev().collect::<Vec<_>>());

    // a slice a `let` keeps holds its temporary to the end of the scope
    {
        let tail = &make(40)[1..];
        println!("k {tail:?}");
        println!("scope end");
    }
    println!("end");
}
