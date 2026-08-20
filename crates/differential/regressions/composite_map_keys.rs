// A tuple, an option of a struct, and an enum as map and set keys, and a
// key past i64::MAX that keeps its width on the way back out. The
// interpreter once accepted only scalars as keys and dropped the width.
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct P {
    x: u8,
    s: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum E {
    A,
    B(i32),
}

fn main() {
    let mut m: HashMap<(u8, char), i32> = HashMap::new();
    m.insert((1, 'a'), 10);
    m.insert((1, 'a'), 11);
    m.insert((2, 'b'), 20);
    let mut pairs: Vec<((u8, char), i32)> = m.clone().into_iter().collect();
    pairs.sort();
    println!("{pairs:?} {:?} {}", m.get(&(2, 'b')), m.contains_key(&(3, 'c')));
    let mut s: HashSet<Option<P>> = HashSet::new();
    s.insert(None);
    s.insert(Some(P { x: 1, s: String::from("q") }));
    s.insert(Some(P { x: 1, s: String::from("q") }));
    let mut v: Vec<Option<P>> = s.into_iter().collect();
    v.sort();
    println!("{v:?}");
    let mut e: HashMap<E, Vec<u8>> = HashMap::new();
    e.entry(E::B(2)).or_default().push(1);
    e.entry(E::A).or_default().push(3);
    e.entry(E::B(2)).or_default().push(4);
    let mut ev: Vec<(E, Vec<u8>)> = e.into_iter().collect();
    ev.sort();
    println!("{ev:?}");
    let h: HashMap<u64, Vec<bool>> = [(18446744073709551614u64, vec![true])].into_iter().collect();
    let mut hv: Vec<(u64, Vec<bool>)> = h.into_iter().collect();
    hv.sort();
    println!("{hv:?}");
    let w: HashSet<(u8, u64)> = [(255u8, 18446744073709551614u64)].into_iter().collect();
    println!("{:?}", w.into_iter().collect::<Vec<(u8, u64)>>());
}
