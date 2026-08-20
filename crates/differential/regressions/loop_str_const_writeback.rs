//! A string literal assigned inside a `for` body ran on the scalar plan,
//! which keeps such a literal as an index into the plan's own string table.
//! That form has no boxed value, so writeback skipped the slot and the
//! variable kept what it held before the loop. Found while fixing the same
//! staleness in a plain copy, see `loop_move_non_scalar.rs`.

fn main() {
    let mut word: &str = "start";
    for _ in 0usize..2usize {
        word = "moved";
    }
    println!("{word}");

    let mut pick: &str = "start";
    let limit: i64 = 3;
    for step in 0i64..limit {
        pick = if step > 0 { "later" } else { "first" };
    }
    println!("{pick}");
}
