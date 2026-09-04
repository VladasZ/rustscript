#!/usr/bin/env rust

// A temporary made for an `if` or `while` condition drops before the branch runs, an `if let`
// scrutinee nobody bound drops before the `else` block, and a loop over a fresh collection owns
// its items. A payload a combinator rejects drops only when the receiver was the caller's own.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug, Clone, Default)]
struct Holder {
    item: D,
}

#[derive(Debug, Clone, Default)]
struct Pair {
    left: D,
    right: D,
}

impl From<i64> for Pair {
    fn from(value: i64) -> Self {
        Pair {
            left: D(value),
            right: D(value + 1),
        }
    }
}

fn some(value: D) -> Option<D> {
    if value.0 < 0 { None } else { Some(value) }
}

fn read(item: &D) -> i64 {
    item.0
}

fn main() {
    let mut pushed = vec![D(1)];
    for item in pushed.clone() {
        pushed.push(item);
    }
    println!("a {}", pushed.len());
    let mut holders = Vec::new();
    for holder in some(D(2))
        .map(|item| Holder { item })
        .into_iter()
        .collect::<Vec<Holder>>()
    {
        println!("b {:?}", holder.item);
        holders.push(holder);
    }
    println!("b {}", holders.len());
    let mapped = some(D(16)).map(|item| item.0 + 1);
    println!("n {mapped:?}");
    let lent_mapped = holders.last().map(|holder| holder.item.0);
    println!("o {lent_mapped:?}");
    for item in vec![D(3); 2].into_iter().rev() {
        println!("c {item:?}");
    }
    if vec![D(4)].len() == 1 {
        println!("d");
    }
    let mut turns = 0;
    while vec![D(5)].len() == 1 && turns < 2 {
        turns += 1;
        println!("e {turns}");
    }
    if let Some(item) = some(D(6)).filter(|item| item.0 > 9) {
        println!("f {item:?}");
    } else {
        println!("f none");
    }
    if let Some(item) = Vec::from([D(7)]).last().filter(|item| item.0 > 9) {
        println!("g {item:?}");
    } else {
        println!("g none");
    }
    let mut stack = vec![D(8), D(9)];
    while let Some(top) = stack.pop() {
        println!("h {top:?}");
    }
    let mut counters = vec![vec![D(10)], vec![D(11)], Vec::new()];
    while let Some(chunk) = counters.pop().filter(|chunk| !chunk.is_empty()) {
        println!("i {chunk:?}");
    }
    let kept: Vec<D> = vec![D(12)].into_iter().skip(1).collect();
    println!("j {}", kept.len());
    let last_len = Vec::from([D(13)]).last().map_or(0, |item| item.0);
    println!("k {last_len}");
    let rejected = some(D(14)).filter(|item| item.0 < 0);
    println!("l {rejected:?}");
    let lent = Vec::from([D(15)]);
    let borrowed = lent.last().filter(|item| item.0 < 0);
    println!("m {borrowed:?}");
    let unused = (lent.len() > 9).then_some(D(17));
    println!("p {unused:?}");
    let first = vec![D(18), D(19)].into_iter().map(|item| item.0).nth(1);
    println!("q {first:?}");
    let pairs = vec![(D(20), 1), (D(21), 2)];
    let mut chained = pairs.into_iter().zip(0..).peekable();
    println!("r {:?}", chained.peek().map(|(pair, index)| pair.1 + index));
    drop(chained);
    let converted: Pair = 22.into();
    println!("s {} {}", converted.left.0, converted.right.0);
    println!("t {}", read(&D(24)));
    let patched = Pair {
        left: D(25),
        ..Default::default()
    };
    println!("u {} {}", patched.left.0, patched.right.0);
    let picked = {
        let mut sorted = vec![D(27), D(26)];
        sorted.sort_by_key(|item| item.0);
        sorted
    }
    .into_iter()
    .map(|item| item.0)
    .nth(1);
    println!("v {picked:?}");
    let mut replaced = vec![D(28)];
    replaced = vec![D(29); 1].into_iter().take(0).collect();
    println!("w {}", replaced.len());
    println!("end");
}
