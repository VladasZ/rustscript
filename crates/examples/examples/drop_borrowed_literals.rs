#!/usr/bin/env rust

//! A tuple, array, `vec!`, enum variant or struct that holds a borrow owns nothing behind it,
//! so the owner keeps its items when the holder goes.

#[derive(Debug, Clone, PartialEq)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct View<'a> {
    items: &'a [D],
}

fn total(parts: &[&[D]]) -> usize {
    parts.iter().map(|part| part.len()).sum()
}

fn main() {
    let a = vec![D(1), D(3)];
    let b = vec![D(2)];
    for part in [a.as_slice(), b.as_slice()] {
        for item in part {
            println!("item {}", item.0);
        }
        println!("first {:?} len {}", part.first().map(|d| d.0), part.len());
    }
    let (left, right) = (a.as_slice(), &b);
    println!("{} {} {}", left.len(), right.len(), left[1].0);
    let parts = vec![a.as_slice(), b.as_slice()];
    println!("{} {}", total(&parts), parts[1][0].0);
    println!("{}", parts[0] == a.as_slice());
    println!(
        "{:?}",
        parts.iter().map(|p| p.len()).collect::<Vec<usize>>()
    );
    let flat: Vec<D> = parts.concat();
    println!("{}", flat.len());
    let some = Some(a.as_slice());
    println!("{:?}", some.map(<[D]>::len));
    let view = View {
        items: a.as_slice(),
    };
    println!("{}", view.items.len());
    let both = [&a, &b];
    println!("{}", both.iter().map(|v| v.len()).sum::<usize>());

    // temporaries, gone when the statement ends
    println!("{}", (a.as_slice(), b.as_slice()).0.len());
    println!("{}", [a.as_slice(), b.as_slice()].len());
    println!("{}", vec![a.as_slice(), b.as_slice()].len());
    println!("{}", total(&[a.as_slice(), b.as_slice()]));
    println!("{}", [a.as_slice(), b.as_slice()].concat().len());
    // clones are the literal's own and drop with it
    println!("{}", [a.clone(), b.clone()].len());
    println!("{} {}", a.len(), b.len());
    println!("end");
}
