//! `*map.entry(k).or_insert(v) += x` writes through the entry reference, and
//! the add panics at i64's bound exactly like the compiled program. The
//! campaign found the interpreter answering `or_insert` with a detached clone
//! and dying on "assignment through a non-reference value" instead.

use std::collections::HashMap;

fn opaque(v: i64) -> i64 {
    v
}

fn main() {
    let mut tally: HashMap<i64, i64> = HashMap::new();
    *tally.entry(opaque(1)).or_insert(opaque(3)) += opaque(4);
    println!("{:?}", tally.get(&1));
    *tally.entry(opaque(1)).or_insert(opaque(0)) += opaque(9223372036854775806);
    println!("{:?}", tally.get(&1));
}
