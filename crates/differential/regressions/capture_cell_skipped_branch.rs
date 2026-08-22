//! A closure in a branch that never runs builds no capture cell. The
//! interpreter once died with "missing mutable capture cell". From seed
//! 20690109027.

use std::collections::HashMap;

fn opaque(value: i64) -> i64 {
    value
}

fn main() {
    let seed: i64 = opaque(7);
    let names: Vec<String> = Vec::new();
    // The closure mutates `seed` and its branch never runs.
    let scores: HashMap<String, i64> = if names.iter().any(|name| name.is_empty()) {
        names
            .iter()
            .map(|name| (name.clone(), seed.rotate_left(0)))
            .collect()
    } else {
        let mut built: HashMap<String, i64> = HashMap::new();
        built.insert(String::from("only"), seed);
        built
    };
    println!("{seed} {:?}", scores.len());

    let mut total: i64 = opaque(3);
    if names.len() > 10 {
        let mut add = |value: i64| {
            total += value;
        };
        add(100);
    }
    println!("{total}");
}
