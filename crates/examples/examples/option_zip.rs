#!/usr/bin/env rust

// `zip` pairs two `Some` values and gives `None` as soon as either side is missing.

fn none_i64() -> Option<i64> {
    None
}

fn main() {
    let number: Option<i64> = Some(4);
    let word: Option<String> = Some(String::from("four"));
    let missing = none_i64();

    println!("{:?}", number.zip(word.clone()));
    println!("{:?}", number.zip(none_i64()));
    println!("{:?}", missing.zip(word.clone()));
    println!("{:?}", none_i64().zip(none_i64()));

    let (count, label) = number.zip(word).unwrap_or_default();
    println!("{count} {label}");

    let pairs: Vec<(i64, i64)> = vec![Some(1), None, Some(3)]
        .into_iter()
        .filter_map(|item| item.zip(Some(10)))
        .collect();
    println!("{pairs:?}");
}
