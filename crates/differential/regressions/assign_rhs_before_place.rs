//! An assignment evaluates its right operand before the place expression, so
//! the right side's panic fires first. The campaign found the interpreter
//! evaluating the `entry(..).or_insert(..)` place first and dying on its
//! multiply overflow where compiled Rust reports the sum's add overflow. From
//! seed 20675014558.

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
