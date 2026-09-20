//! A pattern that binds one part by value and another by `ref` over a fresh value. The by
//! value binding takes its part and drops with the arm, the `ref` one lends its part from the
//! scrutinee, which drops what is left at the end of the statement.

#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct Pair {
    left: Trace,
    right: Trace,
    count: u8,
}

fn make(n: i64) -> Pair {
    Pair {
        left: Trace(n),
        right: Trace(n + 1),
        count: 3,
    }
}

fn wrap(n: i64) -> Option<(Trace, Trace)> {
    Some((Trace(n), Trace(n + 1)))
}

fn main() {
    let seen = match make(1) {
        Pair {
            left: taken,
            right: ref lent,
            ..
        } => {
            println!("arm {} {}", taken.0, lent.0);
            taken.0 + lent.0
        }
    };
    println!("seen {seen}");

    if let Some((taken, ref lent)) = wrap(10) {
        println!("if let {} {}", taken.0, lent.0);
    }
    println!("after if let");

    let total = match make(20) {
        Pair {
            left: ref lent,
            count: n @ 1..=5,
            ..
        } => i64::from(n) + lent.0,
        _ => 0,
    };
    println!("total {total}");

    let mut turns = 0;
    while let Some((taken, ref lent)) = wrap(30 + turns) {
        println!("turn {} {}", taken.0, lent.0);
        turns += 10;
        if turns > 10 {
            break;
        }
    }
    println!("end");
}
