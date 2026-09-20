//! An adapter owns the iterator it wraps. When a closure up the chain panics, the items the
//! source still holds drop with the chain, whatever adapter sits at its end, `step_by`,
//! `skip`, `rev`, `enumerate` and the rest, not only `filter` and `take`.

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
    let sizes: Vec<u8> = vec![Trace(1), Trace(2), Trace(3)]
        .into_iter()
        .map(|item: Trace| {
            let missing = Vec::<u8>::new()[three() + usize::try_from(item.0).unwrap_or(0)];
            missing
        })
        .skip(0usize)
        .enumerate()
        .map(|pair| pair.1)
        .rev()
        .step_by(1usize)
        .collect();
    println!("{sizes:?}");
}
