//! A turbofish collect nested inside an annotated chain must not eat the outer hint. Seed 20675319747.

use std::collections::HashMap;

fn main() {
    let mut source: HashMap<String, i64> = HashMap::new();
    source.insert(String::from("a"), 1);
    source.insert(String::from("b"), 2);
    let rebuilt: HashMap<String, i64> = ((if true {
        source.clone().into_iter().collect::<HashMap<String, i64>>()
    } else {
        HashMap::new()
    })
    .into_iter()
    .collect());
    let mut keys: Vec<String> = rebuilt.clone().into_keys().collect();
    keys.sort();
    println!("keys={keys:?}");
    let mut values: Vec<i64> = rebuilt.into_values().collect();
    values.sort();
    println!("values={values:?}");
}
