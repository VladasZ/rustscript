// A by value binding of `matches!` takes its part and drops at the end of the arm, before the
// statement goes on. A guard reads the binding by reference first.
// Found by seeds 20717202254 and 92000019339.

#[derive(Debug)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    println!("a {}", matches!(Some(T(1)), Some(b)));
    println!("b {}", matches!(Some(T(2)), Some(_)));
    println!("c {}", matches!(Some(T(3)), Some(b) if b.0 > 1));
    println!("d {}", matches!(Some(T(4)), None));
}
