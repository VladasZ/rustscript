//! A string literal assigned in a `for` body must write back on the scalar plan. See
//! `loop_move_non_scalar.rs`.

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
