// A field moved out of a local into a call argument is the argument's own, so a panic in a later
// argument drops it before the locals unwind. Found by seed 20721009909.

struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct S {
    t: T,
}

fn take(first: T, second: T, n: i8) -> i8 {
    println!("got {} {}", first.0, second.0);
    n
}

fn pair() -> (T, T) {
    (T(2), T(3))
}

fn boom() -> i8 {
    let v: Vec<i8> = vec![88, 88];
    v.into_iter().product::<i8>()
}

fn main() {
    let s = S { t: T(5) };
    let p = (T(6), T(7));
    println!("{}", take(s.t, pair().1, 1) + take(p.1, T(8), boom()));
}
