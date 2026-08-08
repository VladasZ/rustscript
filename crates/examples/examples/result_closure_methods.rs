#!/usr/bin/env rust

// The closure taking methods on Result. `map_or_else` is the one that differs
// from the Option form, its fallback receives the error rather than nothing.

fn parse(text: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|e| format!("{text} is not a number, {e}"))
}

fn main() {
    let good = parse("42");
    let bad = parse("x");

    println!("mapped ok: {:?}", good.as_ref().map(|n| n * 2));
    println!("mapped err: {:?}", bad.as_ref().map(|n| n * 2));

    println!("map_or ok: {}", parse("42").map_or(-1, |n| n * 2));
    println!("map_or err: {}", parse("x").map_or(-1, |n| n * 2));

    println!(
        "map_or_else ok: {}",
        parse("42").map_or_else(|e| i64::try_from(e.len()).unwrap(), |n| n * 2)
    );
    println!(
        "map_or_else err: {}",
        parse("x").map_or_else(|e| i64::try_from(e.len()).unwrap(), |n| n * 2)
    );

    println!(
        "map_or_else sees the error: {}",
        parse("x").map_or_else(|e| e, |n| n.to_string())
    );

    println!(
        "unwrap_or_else ok: {}",
        parse("7").unwrap_or_else(|e| i64::try_from(e.len()).unwrap())
    );
    println!(
        "unwrap_or_else err: {}",
        parse("x").unwrap_or_else(|e| i64::try_from(e.len()).unwrap())
    );

    let chained = parse("21").and_then(|n| parse(&(n * 2).to_string()));
    println!("and_then: {chained:?}");
}
