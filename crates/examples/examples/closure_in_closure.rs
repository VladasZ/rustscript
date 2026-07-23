#!/usr/bin/env rust

// A let-bound closure called from inside another closure reaches the callee
// as an upvalue, not a local. The compiler used to handle only the local
// case, so the call fell through to path resolution and died at runtime with
// "unknown function". The shape comes from a real table renderer whose
// per-cell closure was called both directly and from a nested map.

fn main() {
    let lines_of = |word: &str, i: usize| -> String { format!("{word}:{i}") };

    // Direct call in the defining frame, the case that always worked.
    println!("{}", lines_of("direct", 0));

    // Called from inside a nested closure, the callee is an upvalue.
    let words = ["a", "b", "c"];
    let cells: Vec<String> = (0..words.len()).map(|i| lines_of(words[i], i)).collect();
    println!("{}", cells.join(" "));

    // Two levels deep, the capture has to travel up the frame chain.
    let rows: Vec<String> = words
        .iter()
        .map(|w| {
            let inner: Vec<String> = (0..2).map(|i| lines_of(w, i)).collect();
            inner.join(",")
        })
        .collect();
    println!("{}", rows.join(" "));

    // A captured closure that itself captures a local.
    let sep = "-";
    let joined = move |a: &str, b: &str| format!("{a}{sep}{b}");
    let pairs: Vec<String> = words.iter().map(|w| joined(w, "x")).collect();
    println!("{}", pairs.join(" "));
}
