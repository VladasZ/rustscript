//! `or_insert` once answered a detached clone and `+=` died with
//! "assignment through a non-reference value".

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
