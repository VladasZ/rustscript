#!/usr/bin/env rust


fn main() {
    let mut items = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let removed = items.swap_remove(1);
    println!("removed {removed}, left {items:?}");
    let first = items.swap_remove(0);
    println!("removed {first}, left {items:?}");
    let last = items.swap_remove(items.len() - 1);
    println!("removed {last}, left {items:?}");
}
