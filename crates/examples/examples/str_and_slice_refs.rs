#!/usr/bin/env rust

fn pick<T>(a: T, b: T, first: bool) -> T {
    if first { a } else { b }
}

fn longer<'a>(a: &'a str, b: &'a str) -> &'a str {
    // both references go through a by value generic parameter and stay usable after
    let chosen = pick(a, b, a.len() >= b.len());
    println!("{a} or {b} gives {chosen}");
    chosen
}

fn total(items: &[i32]) -> i32 {
    let head = pick(items, items, true);
    head.iter().sum::<i32>() + i32::try_from(items.len()).unwrap_or(0)
}

fn main() {
    let name = String::from("hello world");
    let word: &str = &name[..5];
    let rest = &name[6..];
    println!("{word}|{rest}|{}", &name[3..=7]);
    println!("{}", longer(word, rest));
    println!("{}", longer(name.as_str(), "x"));

    let numbers = vec![1, 2, 3, 4];
    println!(
        "{:?} {:?} {:?}",
        &numbers[1..],
        &numbers[..2],
        &numbers[1..=2]
    );
    println!("{}", total(&numbers));
    println!("{}", total(&numbers[2..]));

    let empty = &[] as &[u8];
    println!("{} {:?}", empty.len(), (&[] as &[String]).to_vec());
    let bytes = &[1u8, 2, 3] as &[u8];
    println!("{bytes:?}");

    match name.strip_prefix("hello") {
        Some(tail) => println!("tail is {:?}", tail.trim()),
        None => println!("no prefix"),
    }
    if let Some((left, right)) = name.split_once(' ') {
        println!("{left} / {right}");
    }
    match word {
        "hello" => println!("greeting"),
        _ => println!("something else"),
    }
}
