#!/usr/bin/env rust

// A closure called from inside another closure is an upvalue, not a local.

fn main() {
    let lines_of = |word: &str, i: usize| -> String { format!("{word}:{i}") };

    // direct call
    println!("{}", lines_of("direct", 0));

    // from a nested closure
    let words = ["a", "b", "c"];
    let cells: Vec<String> = (0..words.len()).map(|i| lines_of(words[i], i)).collect();
    println!("{}", cells.join(" "));

    // 2 levels deep
    let rows: Vec<String> = words
        .iter()
        .map(|w| {
            let inner: Vec<String> = (0..2).map(|i| lines_of(w, i)).collect();
            inner.join(",")
        })
        .collect();
    println!("{}", rows.join(" "));

    // a captured closure that captures a local itself
    let sep = "-";
    let joined = move |a: &str, b: &str| format!("{a}{sep}{b}");
    let pairs: Vec<String> = words.iter().map(|w| joined(w, "x")).collect();
    println!("{}", pairs.join(" "));
}
