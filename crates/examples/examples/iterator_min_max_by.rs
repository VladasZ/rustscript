#!/usr/bin/env rust

//! `min_by` and `max_by` fold with the best so far on the left, `max_by` keeps the later of 2
//! equal items and `min_by` the earlier one. `unzip`, `is_sorted`, `next_if` and `next_if_eq`.

#[derive(Debug, Clone, PartialEq)]
struct Noisy(i64);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn noisy(base: i64) -> Vec<Noisy> {
    (0..4).map(|step| Noisy(base + step)).collect()
}

fn main() {
    let largest = noisy(40)
        .into_iter()
        .max_by(|left, right| (left.0 % 3).cmp(&(right.0 % 3)));
    println!("max_by {largest:?}");
    let smallest = noisy(50)
        .into_iter()
        .min_by(|left, right| (left.0 % 3).cmp(&(right.0 % 3)));
    println!("min_by {smallest:?}");

    let words = ["pear", "fig", "plum", "kiwi"];
    println!(
        "by len {:?} {:?}",
        words
            .iter()
            .max_by(|left, right| left.len().cmp(&right.len())),
        words
            .iter()
            .min_by(|left, right| left.len().cmp(&right.len()))
    );
    let floats = [2.5f64, -1.0, 9.25];
    println!(
        "floats {:?}",
        floats.iter().max_by(|left, right| left.total_cmp(right))
    );
    println!(
        "empty {:?}",
        Vec::<u8>::new()
            .iter()
            .min_by(|left, right| right.cmp(left))
    );

    let (items, ids): (Vec<Noisy>, Vec<i64>) = noisy(60)
        .into_iter()
        .map(|item| {
            let id = item.0;
            (item, id)
        })
        .unzip();
    println!("unzip {items:?} {ids:?}");
    let (names, sizes): (Vec<&str>, Vec<usize>) =
        words.iter().map(|word| (*word, word.len())).unzip();
    println!("unzip refs {names:?} {sizes:?}");

    let letters: Vec<char> = "ace".chars().collect();
    println!(
        "is_sorted {} {} {} {}",
        [1, 2, 2, 5].iter().is_sorted(),
        [1, 3, 2].iter().is_sorted(),
        [0.5, f64::NAN].iter().is_sorted(),
        letters.is_sorted()
    );

    let mut queue = noisy(70).into_iter().peekable();
    while let Some(item) = queue.next_if(|item| item.0 < 72) {
        println!("next_if {item:?}");
    }
    println!("next_if_eq {:?}", queue.next_if_eq(&Noisy(72)));
    println!("next_if_eq miss {:?}", queue.next_if_eq(&Noisy(99)));
    println!("left {:?}", queue.peek());
    println!("end");
}
