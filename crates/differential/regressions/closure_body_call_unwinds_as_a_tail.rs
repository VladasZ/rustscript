//! A closure whose body is a bare call is a tail call like a block's, so a panic in an
//! argument drops the temporaries of the arguments before the owned receiver and operands.

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn idx() -> usize {
    3
}

fn main() {
    let v: Vec<T> = (0..1)
        .map(|_| Some(T(20)).and(Some(T(21))).unwrap_or(vec![T(26)][idx()].clone()))
        .collect();
    println!("{v:?}");
}
