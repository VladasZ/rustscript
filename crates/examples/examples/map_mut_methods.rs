use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn main() {
    let mut m: HashMap<&str, i32> = HashMap::from([("a", 1)]);
    m.extend([("b", 2)]);
    m.extend(
        vec![("c", 3), ("a", 10)]
            .into_iter()
            .filter(|(_, v)| *v > 2),
    );
    for v in m.values_mut() {
        *v += 1;
    }
    let mut pairs: Vec<(&str, i32)> = m.iter().map(|(k, v)| (*k, *v)).collect();
    pairs.sort_unstable();
    println!("{pairs:?}");
    m.retain(|k, v| {
        *v *= 2;
        *k != "b"
    });
    let mut pairs: Vec<(&str, i32)> = m.into_iter().collect();
    pairs.sort_unstable();
    println!("{pairs:?}");

    let mut b: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    b.extend([("z".to_string(), vec![1]), ("y".to_string(), vec![2, 3])]);
    let other: BTreeMap<String, Vec<u8>> = BTreeMap::from([("x".to_string(), vec![])]);
    b.extend(other);
    for (k, v) in &mut b {
        v.push(u8::try_from(k.len()).unwrap_or(u8::MAX));
    }
    b.retain(|_, v| v.len() > 1);
    println!("{b:?}");

    let mut s: BTreeSet<i32> = BTreeSet::from([5, 1]);
    s.extend([3, 1, 9]);
    s.retain(|x| x % 3 != 0);
    println!("{s:?}");

    let mut h: HashSet<char> = HashSet::new();
    h.extend("hello".chars());
    println!("{}", h.len());
}
