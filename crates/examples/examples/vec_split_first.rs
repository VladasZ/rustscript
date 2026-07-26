#!/usr/bin/env rust

// split_first hands back the head and the rest in one step. This is the shape a
// command's output takes when the first line is the thing being looked for and
// the rest are the candidates, as `readlink -f target a b c` returns.

fn resolved(out: &str) -> String {
    let lines: Vec<String> = out.lines().map(str::trim).map(str::to_string).collect();
    match lines.split_first() {
        Some((target, rest)) => match rest.iter().position(|p| p == target) {
            Some(i) => format!("{target} is candidate {i}"),
            None => format!("{target} is none of the {} candidates", rest.len()),
        },
        None => "nothing to resolve".to_string(),
    }
}

fn main() {
    println!("{}", resolved("/dev/ttyUSB2\n/dev/ttyUSB0\n/dev/ttyUSB2\n"));
    println!("{}", resolved("/dev/ttyUSB9\n/dev/ttyUSB0\n/dev/ttyUSB2\n"));
    println!("{}", resolved(""));

    let single: Vec<String> = "only".lines().map(str::to_string).collect();
    let (head, rest) = single.split_first().unwrap();
    println!("head {head}, rest empty {}", rest.is_empty());

    let empty: Vec<String> = Vec::new();
    println!("empty is none: {}", empty.split_first().is_none());
}
