// `Option::zip` takes an option, never an iterator, so a fresh `Some` argument stays a `Some`.
// Found by seed 92000206610.

#[derive(Debug, Default)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    println!("{:?}", Some(String::from("a")).zip(Some(String::from("b"))));
    println!("{:?}", Some(T(1)).zip(Some(T(2))).unwrap_or_default());
    println!("{:?}", Some(T(3)).zip(None::<T>));
}
