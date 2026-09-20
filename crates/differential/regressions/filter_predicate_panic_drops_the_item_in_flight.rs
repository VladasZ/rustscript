//! A `filter` predicate only borrows the item. When it panics the item is still the
//! iterator's, so it drops as the adapter unwinds, after the locals of the predicate and
//! before the items the source still holds.

#[derive(Clone, Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn three() -> usize {
    3
}

fn main() {
    let kept: Vec<Trace> = vec![Trace(1), Trace(2)]
        .into_iter()
        .filter(|item| {
            let copy = Trace(item.0 + 10);
            Vec::<bool>::new()[three() + usize::try_from(copy.0).unwrap_or(0)]
        })
        .collect();
    println!("{kept:?}");
}
