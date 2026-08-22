//! The right operand of an assignment evaluates before the place, so `entry(..).or_insert(..)`
//! must not panic first. Seed 20675014558.

use std::collections::HashMap;

fn opaque(v: i64) -> i64 {
    v
}

fn main() {
    let mut tally: HashMap<String, i64> = HashMap::new();
    for key in vec![String::from("a"), String::from("b")] {
        *tally.entry(key.clone()).or_insert(opaque(i64::MIN) * opaque(i64::MAX - 1)) +=
            vec![opaque(3636640975612606248), opaque(6854043497958475798)]
                .iter()
                .copied()
                .sum::<i64>();
    }
    println!("{:?}", tally.len());
}
