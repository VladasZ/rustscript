#!/usr/bin/env rust

// A closure parameter taken by value drops at the closure's end, one lent by an `iter()`
// adapter does not, and a consuming terminal drops what it throws away.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let sevens: Vec<u16> = vec![D(1), D(2)].into_iter().map(|_x: D| 7u16).collect();
    println!("sevens {sevens:?}");
    let show = |x: D| println!("each {}", x.0);
    vec![D(3), D(4)].into_iter().for_each(show);
    println!("shown");
    let source = vec![D(5), D(6)];
    let bumped: Vec<i64> = source.into_iter().map(|x: D| x.0 + 1).collect();
    println!("bumped {bumped:?}");
    let counted = vec![D(7), D(8)];
    let count = counted.into_iter().map(|x| x.0).filter(|n| *n > 7).count();
    println!("count {count}");
    let sum: i64 = vec![D(9), D(10)].into_iter().map(|x: D| x.0).sum();
    println!("sum {sum}");
    let largest = vec![D(11), D(12)].into_iter().map(|x: D| x.0).max();
    println!("largest {largest:?}");
    let last = vec![D(13), D(14)].into_iter().last();
    println!("last {last:?}");
    let folded = vec![D(15), D(16)]
        .into_iter()
        .fold(D(0), |acc: D, x: D| if x.0 > acc.0 { x } else { acc });
    println!("folded {folded:?}");
    let kept: Vec<D> = vec![D(17), D(18), D(19)]
        .into_iter()
        .filter(|d| d.0 != 18)
        .collect();
    println!("kept {kept:?}");
    let lent = vec![D(20), D(21)];
    let total: i64 = lent.iter().map(|d| d.0).sum();
    println!("total {total} {lent:?}");
    let second = vec![D(22), D(23), D(24)].into_iter().nth(1);
    println!("second {second:?}");
    let found = vec![D(25), D(26)].into_iter().find(|d| d.0 == 26);
    println!("found {found:?}");
    println!("end");
}
