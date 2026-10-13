#!/usr/bin/env rust

//! `first_chunk` and `last_chunk` hand out the first or the last `N` items of a slice as an
//! array. `split_first_chunk` and `split_last_chunk` hand out the rest too. Each is `None` when
//! the slice is shorter than `N`.

fn main() {
    let parts: Vec<&str> = "a b c d".split(' ').collect();
    if let Some(&[first, second, third]) = parts.first_chunk::<3>() {
        println!("{first} {second} {third}");
    }
    if let Some([left, right]) = parts.last_chunk::<2>() {
        println!("{left} {right}");
    }
    println!("{:?}", parts.first_chunk::<4>());
    println!("{:?}", parts.first_chunk::<5>());
    println!("{:?}", parts.last_chunk::<0>());
    println!("{}", parts.last_chunk::<5>().is_none());

    let bytes: &[u8] = &[1, 2, 3, 4, 5, 6];
    if let Some((head, rest)) = bytes.split_first_chunk::<4>() {
        println!("{} {rest:?}", u32::from_be_bytes(*head));
    }
    if let Some((rest, tail)) = bytes.split_last_chunk::<2>() {
        println!("{rest:?} {}", u16::from_le_bytes(*tail));
    }
    println!("{:?}", bytes.split_first_chunk::<6>());
    println!("{:?}", bytes.split_first_chunk::<7>());
    println!("{:?}", bytes.split_last_chunk::<0>());
    println!("{:?}", bytes.split_last_chunk::<7>());

    let sum: u32 = bytes
        .first_chunk::<3>()
        .map_or(0, |chunk| chunk.iter().map(|byte| u32::from(*byte)).sum());
    println!("{sum}");

    let words: Vec<String> = parts.iter().take(2).map(|part| part.repeat(2)).collect();
    let tail = words.last_chunk::<1>().map(|chunk| chunk.to_vec());
    println!("{tail:?} {words:?}");
    let empty: Vec<String> = Vec::new();
    println!(
        "{:?} {:?}",
        empty.first_chunk::<0>(),
        empty.first_chunk::<1>()
    );
}
