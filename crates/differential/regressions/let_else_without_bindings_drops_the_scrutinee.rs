//! A `let else` whose pattern binds nothing, `None` or `Some(_)`. The scrutinee is a temporary
//! of the statement. It drops before the `else` runs when the pattern misses, and at the
//! semicolon when it holds.

#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make(n: i64) -> Option<Trace> {
    Some(Trace(n))
}

fn none() -> Option<Trace> {
    None
}

fn run(step: i64) -> i64 {
    // nothing bound, the match holds and the temporary ends with the statement
    let Some(_) = make(step) else {
        return 0;
    };
    println!("after the wild let");
    let None = none() else {
        return 1;
    };
    // a by value binding lives to the end of the function
    let Some(kept) = make(step + 1) else {
        return 2;
    };
    println!("kept {}", kept.0);
    // the pattern misses, the scrutinee drops before the else runs
    let None = make(step + 2) else {
        println!("else of step {step}");
        return 3;
    };
    4
}

fn main() {
    println!("run {}", run(10));
    for turn in 0..2i64 {
        let None = make(20 + turn) else {
            println!("continue {turn}");
            continue;
        };
        println!("unreachable {turn}");
    }
    println!("end");
}
