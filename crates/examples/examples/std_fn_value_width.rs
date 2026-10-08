#!/usr/bin/env rust

// A std function passed by name keeps its return type, so an integer behind it keeps its width.

fn main() {
    let missing: Option<&str> = None;
    let name = Some("config.toml");

    println!("{}", !missing.map(String::from).map_or(0, |s| s.len()));
    println!("{}", !name.map(String::from).map_or(0, |s| s.len()));
    println!("{}", !missing.map(str::to_string).map_or(0, |s| s.len()));
    println!(
        "{}",
        !name.map(str::to_owned).as_deref().map_or(0, str::len)
    );
    println!("{}", !name.map_or(0, str::len));
    println!("{}", !missing.map_or(0, str::len));

    let counts: Vec<usize> = vec![1, 2];
    let bytes: Vec<u8> = vec![1, 2];
    let count = counts.first().copied();
    let small = bytes.first().copied();
    println!("{}", !u64::try_from(counts[0]).unwrap_or(0));
    println!(
        "{}",
        !count.map(u64::try_from).map_or(0, |n| n.unwrap_or(0))
    );
    println!("{}", !small.map_or(0, u64::from));
    println!("{:?}", small.map(u32::from).map(|n| !n));
    println!("{:?}", small.map(i16::from).map(|n| !n));
    let wide: Vec<u64> = bytes.iter().copied().map(u64::from).map(|n| !n).collect();
    println!("{wide:?}");
}
