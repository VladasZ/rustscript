// `match (v)` reads the local like `match v`. What the arms did not move stays with the local
// and drops where it was declared, on a panic too.
// Found by seed 91000214306.

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct S {
    a: u8,
    t: T,
}

fn main() {
    let v = S { a: 0, t: T(0) };
    let v: T = (match (v) {
        S { a: 128, t } => vec![t],
        _ => Vec::<T>::new(),
    })[2]
        .clone();
    println!("{:?}", v);
}
