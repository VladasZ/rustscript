//! A let chain whose later term misses drops what the terms before it bound, and the fresh
//! scrutinee of the term that missed, in reverse order and before the `else` runs. A pattern
//! that misses half way binds nothing, so its scrutinee drops whole and only once.

struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn yes() -> bool {
    true
}

fn make(n: i64) -> Option<Trace> {
    Some(Trace(n))
}

fn pair(n: i64, flag: u8) -> (Trace, u8) {
    (Trace(n), flag)
}

fn main() {
    if let Some(a) = make(1) && !yes() {
        println!("then {}", a.0);
    } else {
        println!("else 1");
    }
    if let None = make(2) {
        println!("then");
    } else {
        println!("else 2");
    }
    if let Some(a) = make(3) && let None = make(4) {
        println!("then {}", a.0);
    } else {
        println!("else 3");
    }
    if let Some(a) = make(5) && !yes() {
        println!("then {}", a.0);
    }
    println!("after 5");
    if let Some(a) = make(6) && yes() && let Some(b) = make(7) {
        println!("then {} {}", a.0, b.0);
    } else {
        println!("else 6");
    }
    // the tuple binds its first slot before the literal misses
    if let Some(a) = make(8) && let (b, 1u8) = pair(9, 0) {
        println!("then {} {}", a.0, b.0);
    } else {
        println!("else 8");
    }
    // every turn starts from clean registers
    for turn in 0..3i64 {
        if let Some(a) = make(10 + turn) && turn == 1 && let Some(b) = make(20 + turn) {
            println!("then {} {}", a.0, b.0);
        } else {
            println!("else turn {turn}");
        }
    }
    println!("end");
}
