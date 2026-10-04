#!/usr/bin/env rust

//! A key of a `BTreeSet` or a `BTreeMap` with a `Drop` impl drops where real Rust drops it,
//! at the end of the scope, on a repeated insert, on `remove`, `clear`, `retain` and when a
//! collect meets the same key twice. A range drops the bounds it took.

use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Noisy(i64);
impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    {
        println!("-- scope");
        let s: BTreeSet<Noisy> = [Noisy(5), Noisy(7), Noisy(5)].into_iter().collect();
        println!("{s:?}");
    }
    {
        println!("-- insert remove");
        let mut s = BTreeSet::new();
        println!("{} {}", s.insert(Noisy(1)), s.insert(Noisy(1)));
        s.insert(Noisy(2));
        println!("{}", s.remove(&Noisy(1)));
        println!("len {}", s.len());
    }
    {
        println!("-- for ref");
        let s: BTreeSet<Noisy> = [Noisy(11), Noisy(10)].into_iter().collect();
        for x in &s {
            println!("see {x:?}");
        }
        println!("-- for owned");
        for x in s {
            println!("own {x:?}");
        }
        println!("after");
    }
    {
        println!("-- into vec");
        let s: BTreeSet<Noisy> = [Noisy(21), Noisy(20)].into_iter().collect();
        let v: Vec<Noisy> = s.into_iter().collect();
        println!("{v:?}");
    }
    {
        println!("-- cloned");
        let s: BTreeSet<Noisy> = [Noisy(31), Noisy(30)].into_iter().collect();
        let v: Vec<Noisy> = s.iter().cloned().collect();
        println!("{v:?} {:?} {:?}", s.first(), s.range(Noisy(31)..).count());
    }
    {
        println!("-- pop clear retain");
        let mut s: BTreeSet<Noisy> = (40..45).map(Noisy).collect();
        println!("{:?}", s.pop_first());
        s.retain(|t| t.0 != 42);
        println!("retained");
        s.clear();
        println!("cleared");
    }
    {
        println!("-- map");
        let mut m: BTreeMap<Noisy, Noisy> = BTreeMap::new();
        m.insert(Noisy(50), Noisy(150));
        println!("{:?}", m.insert(Noisy(50), Noisy(151)));
        m.entry(Noisy(50)).or_insert(Noisy(152));
        m.entry(Noisy(51)).or_insert(Noisy(153));
        m.entry(Noisy(51)).or_insert_with(|| Noisy(154));
        println!("{:?}", m.remove(&Noisy(50)));
        println!("{m:?}");
    }
    {
        println!("-- collect map dup");
        let m: BTreeMap<Noisy, Noisy> = [
            (Noisy(61), Noisy(161)),
            (Noisy(60), Noisy(160)),
            (Noisy(61), Noisy(162)),
        ]
        .into_iter()
        .collect();
        println!("{m:?}");
        let mut e: BTreeSet<Noisy> = BTreeSet::new();
        e.extend([Noisy(70), Noisy(70), Noisy(71)]);
        println!("{e:?}");
    }
    {
        println!("-- range bounds");
        let empty: BTreeSet<Vec<Noisy>> = BTreeSet::new();
        let bound = vec![Noisy(81), Noisy(82)];
        println!("{:?}", empty.range(bound..).count());
        let set: BTreeSet<Noisy> = [Noisy(85), Noisy(87)].into_iter().collect();
        let low = Noisy(84);
        println!("{:?}", set.range(low..Noisy(86)).count());
    }
    println!("end");
}
