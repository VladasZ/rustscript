//! A loop body that copies a non-scalar variable ran on the scalar plan,
//! where such a value loads as an unreadable poison. The copy stored the
//! poison, and a poisoned slot skips writeback, so the assigned variable
//! kept its value from before the loop. The campaign found a `char` copy
//! this way, from seed 20685011353.

fn main() {
    let source: char = ' ';
    let mut target: char = '9';
    for _ in 0usize..3usize {
        target = source;
    }
    println!("{target:?}");

    let text: String = String::from("hi");
    let mut copied: String = String::from("no");
    let mut index: usize = 0;
    for _ in 0usize..2usize {
        copied = text.clone();
        index += 1;
    }
    println!("{copied} {index}");

    let items: Vec<i64> = vec![1, 2];
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0usize..1usize {
        seen = items.clone();
    }
    println!("{seen:?}");
}
