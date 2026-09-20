//! A binding whose value comes out of a `match` arm, an `if` branch or a block as the
//! result of one of the script's own methods owns that value, so the scope end drops it.

#[derive(Clone, Debug)]
struct Trace(i64);
impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
#[derive(Clone, Debug)]
struct S {
    f: Trace,
}
impl S {
    fn again(&self) -> Self {
        S { f: Trace(self.f.0 + 1) }
    }
}
fn one() -> u8 {
    1
}
fn main() {
    let a: S = match one() {
        n => (S { f: Trace(10) }).again(),
    };
    println!("a {}", a.f.0);
    let b: S = (S { f: Trace(20) }).again();
    println!("b {}", b.f.0);
    let c: S = match (one(), String::from("")) {
        (x, y) => (S { f: Trace(30) }).again(),
    };
    println!("c {}", c.f.0);
    let d: S = if one() == 1 { (S { f: Trace(40) }).again() } else { S { f: Trace(0) } };
    println!("d {}", d.f.0);
}
