//! A non scalar copy in a loop body once loaded as poison on the scalar plan
//! and skipped writeback, so the variable kept its old value. From seed
//! 20685011353.

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
