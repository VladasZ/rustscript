//! A temporary inside a `break value` expression drops when the `break` leaves, before the
//! scopes it exits. The jump skips the semicolon that would end it otherwise.

#[derive(Clone)]
struct Trace(i64);
impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn yes() -> bool {
    true
}
fn main() {
    let t = Trace(1);
    // the temporary in the break value drops when the break leaves
    let a: i64 = loop {
        if yes() {
            break (match (Some(t.clone())) { _ => 5, });
        }
    };
    println!("a {a}");
    let b: i64 = loop {
        break Trace(2).0 + 1;
    };
    println!("b {b}");
    let mut n = 0;
    let c: i64 = loop {
        n += 1;
        if n > 2 {
            break Trace(3).0;
        }
    };
    println!("c {c}");
    println!("end");
}
