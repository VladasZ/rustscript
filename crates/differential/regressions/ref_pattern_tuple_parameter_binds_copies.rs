// A tuple pattern behind `&` in a closure parameter binds copies. The closure hands its copy
// of the key to a vec, and the vec drops it, so the key in the map must stay whole.
// Found by seed 20737117830, shrunk by hand. The interpreter panicked with `invalid map key`.

use std::collections::HashMap;

struct DiffTrace(i64);

impl Drop for DiffTrace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let trace = DiffTrace(1);
    println!("{}", trace.0);
    let mut source: HashMap<Option<i16>, i64> = HashMap::new();
    source.insert(Some(1i16), 2i64);
    let kept = source
        .into_iter()
        .filter(|&(key, value): &(Option<i16>, i64)| (vec![key; 3usize].len() as i64) > value)
        .collect::<HashMap<Option<i16>, i64>>();
    println!("{kept:?}");
}
