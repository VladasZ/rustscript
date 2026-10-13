// A map, a set or a vec that holds references to a type with `Drop` drops nothing of the
// owner, for every way the references get in.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn collected(v: &[T], w: &[T]) {
    let set: BTreeSet<&T> = v.iter().collect();
    println!("set {}", set.len());
    let map = v.iter().map(|t| (t, t.0)).collect::<BTreeMap<_, _>>();
    println!("map {}", map.len());
    let hashed = v.iter().map(|t| (t, t.0)).collect::<HashMap<_, _>>();
    println!("hash key {}", hashed.len());
    let values = v.iter().map(|t| (t.0, t)).collect::<BTreeMap<_, _>>();
    println!("value ref {:?}", values);
    let both = v.iter().map(|t| (t, t)).collect::<BTreeMap<_, _>>();
    println!("both {:?}", both);
    let typed: Vec<(&T, i64)> = v.iter().map(|t| (t, t.0)).collect();
    println!("pairs {:?}", typed);
    let pairs = v.iter().map(|t| (t, t.0)).collect::<Vec<_>>();
    println!("pairs {:?}", pairs);
    let some: BTreeMap<&T, usize> = v
        .iter()
        .filter_map(|t| if t.0 > 0 { Some((t, 1usize)) } else { None })
        .collect();
    println!("filter_map {:?}", some);
    let bare = v.iter().filter_map(|t| Some(t)).collect::<BTreeSet<_>>();
    println!("filter_map set {:?}", bare);
    let same = v.iter().map(|t| t).collect::<BTreeSet<_>>();
    println!("identity {:?}", same);
    let zipped = v.iter().zip(w.iter()).collect::<BTreeMap<_, _>>();
    println!("zip {:?}", zipped);
    let counted = v.iter().zip(0i64..).collect::<BTreeMap<_, _>>();
    println!("zip range {:?}", counted);
    let listed = v.iter().zip(0i64..).collect::<Vec<_>>();
    println!("zip vec {:?}", listed);
    let numbered: BTreeMap<usize, &T> = v.iter().enumerate().collect();
    println!("enumerate {:?}", numbered);
    let swapped = v.iter().enumerate().map(|(i, t)| (t, i)).collect::<BTreeMap<_, _>>();
    println!("enumerate swap {:?}", swapped);
    let mixed = v.iter().map(|t| (t, T(t.0 + 10))).collect::<BTreeMap<_, _>>();
    println!("mixed {:?}", mixed);
}

fn inserted(v: &[T], w: &[T]) {
    let folded = v.iter().fold(BTreeMap::new(), |mut m, t| {
        m.insert(t, t.0);
        m
    });
    println!("fold {:?}", folded);
    let seen = v.iter().fold(HashSet::new(), |mut s, t| {
        s.insert(t);
        s
    });
    println!("fold set {}", seen.len());
    let mut extended = BTreeMap::new();
    extended.extend(v.iter().map(|t| (t, t.0)));
    println!("extend {:?}", extended);
    let mut set = BTreeSet::new();
    set.extend(v.iter());
    println!("extend set {:?}", set);
    let mut looped = BTreeMap::new();
    for t in v {
        looped.insert(t, 1u8);
    }
    println!("loop {:?}", looped);
    let mut by_entry: BTreeMap<&T, usize> = BTreeMap::new();
    for t in v.iter() {
        *by_entry.entry(t).or_insert(0) += 1;
    }
    println!("entry {:?}", by_entry);
    let mut unique = HashSet::new();
    for t in v {
        if !unique.insert(t) {
            println!("seen {:?}", t);
        }
    }
    println!("loop set {}", unique.len());
    let mut edited = v.iter().map(|t| (t, t.0)).collect::<BTreeMap<_, _>>();
    println!("removed {:?}", edited.remove(&T(4)));
    edited.insert(&w[0], 7);
    edited.retain(|k, _| k.0 != 1);
    println!("first {:?}", edited.clone().into_iter().next());
    edited.clear();
    println!("cleared {}", edited.len());
}

fn main() {
    let v = vec![T(5), T(4), T(5)];
    let w = vec![T(1), T(2), T(3)];
    collected(&v, &w);
    println!("between");
    inserted(&v, &w);
    println!("end {:?} {:?}", v, w);
}
