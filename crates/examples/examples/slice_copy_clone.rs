#!/usr/bin/env rust

//! `copy_from_slice` and `clone_from_slice` write through a range of a vec. The written items
//! share nothing with the source, so the source going out of scope leaves them whole.

#[derive(Debug, Clone)]
struct Loud(u8);

impl Drop for Loud {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn copied() -> Vec<(bool, u64)> {
    let mut out = vec![(true, 1u64), (true, 2)];
    let source = [(false, 0u64)];
    out[..1].copy_from_slice(&source[..1]);
    out
}

fn main() {
    let loud = Loud(1);
    println!("{:?}", copied());
    println!("{:?}", {
        let mut out = vec![(1u8, Some('a')), (2, None)];
        let source = [(9u8, None), (8, Some('z'))];
        let count = out.len().min(source.len());
        out[..count].copy_from_slice(&source[..count]);
        out
    });

    let mut names = vec![String::from("a"), String::from("b"), String::from("c")];
    {
        let source = [String::from("x"), String::from("y")];
        names[1..].clone_from_slice(&source);
    }
    println!("{names:?}");

    let mut pairs = vec![(1u8, vec![1u8]), (2, vec![2])];
    {
        let source = [(7u8, vec![7u8, 7])];
        pairs[1..=1].clone_from_slice(&source);
    }
    println!("{pairs:?}");

    // a replaced item drops as it is written over, a temporary source at the end of the statement
    let mut loud_items = vec![Loud(2), Loud(3), Loud(4)];
    let source = [Loud(7), Loud(8)];
    loud_items[..2].clone_from_slice(&source[..2]);
    println!("{loud_items:?}");
    loud_items[2..].clone_from_slice(&[Loud(9)]);
    println!("{loud_items:?}");

    let mut bytes = [0u8; 4];
    bytes[1..3].copy_from_slice(&[5, 6]);
    println!("{bytes:?} {}", loud.0);
}
