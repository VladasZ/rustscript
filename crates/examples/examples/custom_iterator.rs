#!/usr/bin/env rust

// A user `Iterator` impl drives for loops and adaptor chains.

struct Counter {
    n: u32,
}

impl Iterator for Counter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.n < 4 {
            self.n += 1;
            Some(self.n)
        } else {
            None
        }
    }
}

fn main() {
    for x in (Counter { n: 0 }) {
        println!("{x}");
    }
    let doubled: Vec<u32> = Counter { n: 0 }.map(|x| x * 2).collect();
    println!("{doubled:?}");
    let sum: u32 = Counter { n: 0 }.filter(|x| x % 2 == 0).sum();
    println!("{sum}");
    let mut c = Counter { n: 2 };
    println!("{:?} {:?}", c.next(), c.next());
}
