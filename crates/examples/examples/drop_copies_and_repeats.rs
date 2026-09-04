#!/usr/bin/env rust

// `vec![x; n]` moves the item into the last slot and clones the rest, `concat` and `extend`
// clone what they copy out of a borrow, and a clone of an enum owns its payload.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn write(target: &mut Result<u32, String>) {
    let cur: Result<u32, String> = target.clone();
    *target = cur.clone().or(Err(String::from("none")));
}

fn main() {
    let twice = vec![D(2); 2];
    println!("twice {twice:?}");
    let none = vec![D(3); 0];
    println!("none {none:?}");
    let mapped: Vec<u8> = vec![D(4); 2].into_iter().map(|_d: D| 1u8).collect();
    println!("mapped {mapped:?}");
    let mut result = Ok::<u32, String>(1u32);
    write(&mut result);
    result = result.clone();
    println!("result {result:?}");
    let original = Ok::<D, String>(D(5));
    let copy = original.clone();
    println!("original {original:?} copy {copy:?}");
    let flat = Vec::from([vec![D(6), D(7)]]).concat();
    println!("flat {flat:?}");
    let joined = [vec![D(8)], vec![D(9)]].concat();
    println!("joined {joined:?}");
    let mut extended = vec![D(10)];
    let more = vec![D(11)];
    extended.extend_from_slice(&more);
    println!("extended {extended:?} more {more:?}");
    let mut owned = vec![D(12)];
    let mut tail = vec![D(13)];
    owned.append(&mut tail);
    println!("owned {owned:?} tail {tail:?}");
    println!("end");
}
