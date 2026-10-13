// A method that takes its argument by value keeps it, hands it back or throws it away. A
// reference that goes in, `held.push(&x)` or `opt.unwrap_or(&fallback)`, drops nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug)]
struct Holder<'a> {
    item: &'a T,
    weight: i64,
}

fn kept(v: &[T], x: &T) {
    let mut held = Vec::new();
    held.push(x);
    held.push(&v[0]);
    held.push(v.first().unwrap());
    println!("push {:?}", held);
    let mut map = BTreeMap::new();
    map.insert(&v[0], 7);
    map.insert(x, 8);
    map.insert(&v[2], 9);
    map.retain(|k, _| k.0 != 9);
    println!("insert {:?}", map);
    let mut set = BTreeSet::new();
    println!("{} {} {}", set.insert(x), set.insert(&v[1]), set.insert(x));
    let mut values = BTreeMap::new();
    values.insert(1, x);
    values.insert(1, &v[0]);
    println!("values {:?}", values);
    let mut queue = VecDeque::new();
    let mut pairs = Vec::new();
    let mut holders = Vec::new();
    for t in v {
        queue.push_back(t);
        queue.push_front(t);
        pairs.push((t, 1u8));
        holders.push(Holder { item: t, weight: t.0 * 2 });
    }
    println!("loop {:?} {:?} {:?}", queue, pairs, holders);
    let total: i64 = holders.iter().map(|h| h.item.0 + h.weight).sum();
    println!("total {total}");
    let mut seen = BTreeSet::new();
    println!("unique {}", v.iter().all(|t| seen.insert(t)));
    let mut each = Vec::new();
    v.iter().for_each(|t| each.push(t));
    each.sort();
    println!("each {:?} {}", each, each.contains(&&T(4)));
    let owned: Vec<T> = each.into_iter().cloned().collect();
    println!("owned {:?}", owned);
}

fn handed_back(v: &[T], fallback: &T) {
    let mut index = BTreeMap::new();
    index.insert(4, T(40));
    let found = index.get(&4).unwrap_or(fallback);
    let missing = index.get(&9).unwrap_or(fallback);
    println!("unwrap_or {:?} {:?}", found, missing);
    let picked: Vec<Option<&T>> = v.iter().map(|t| (t.0 > 4).then_some(t)).collect();
    println!("then_some {:?}", picked);
    let hits: Vec<T> = v.iter().filter_map(|t| (t.0 > 4).then_some(t)).cloned().collect();
    println!("hits {:?}", hits);
    let firsts: Vec<&T> = v.iter().map(|t| index.get(&t.0).unwrap_or(t)).collect();
    println!("firsts {:?}", firsts);
    println!("or {:?}", index.get(&7).or(Some(fallback)));
    let mut slot: Option<&T> = None;
    let mut best: Option<&T> = None;
    for t in v {
        println!("{:?} {:?}", slot.replace(t), best.get_or_insert(t));
    }
    println!("max {:?}", v.iter().map(|t| t).max());
}

fn main() {
    let v = vec![T(5), T(4), T(5)];
    let x = T(9);
    kept(&v, &x);
    println!("between");
    handed_back(&v, &x);
    println!("end {:?} {:?}", v, x);
}
