#!/usr/bin/env rust

// Span keyed map work inside scalar `for` plans. The map seeded before its
// loop proves the plan's borrowed probes hash like generic keys.

use std::collections::HashMap;

fn word_counts() -> Vec<(String, i64)> {
    let text = "pear plum pear apple plum pear fig apple apple pear";
    let mut counts: HashMap<String, i64> = HashMap::new();
    // Seeded outside any plan.
    counts.insert("pear".to_string(), 100);
    for w in text.split_whitespace() {
        let n = counts.get(w).copied().unwrap_or(0) + 1;
        counts.insert(w.to_string(), n);
    }
    let mut pairs: Vec<(String, i64)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| {
        if a.1 == b.1 {
            a.0.cmp(&b.0)
        } else {
            b.1.cmp(&a.1)
        }
    });
    pairs
}

fn token_totals() -> (i64, usize, i64) {
    let text = "id7 x id9 yy id7 zzz id10 id9 id7 w id10101";
    let regex = regex::Regex::new(r"id\d+").unwrap();
    let mut counts: HashMap<String, i64> = HashMap::new();
    let mut total = 0;
    let mut width = 0;
    for found in regex.find_iter(text) {
        let token = found.as_str();
        let count = counts.get(token).copied().unwrap_or(0) + 1;
        counts.insert(token.to_string(), count);
        total += 1;
        width += i64::try_from(found.end() - found.start()).unwrap();
    }
    (total, counts.len(), width)
}

fn json_sums() -> (i64, i64) {
    let text = r#"[{"id":1,"value":10},{"id":2,"value":20},{"id":3,"value":30},{"id":4,"value":40},{"id":5,"value":50}]"#;
    let items: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
    let mut sum = 0;
    let mut ids = 0;
    for it in &items {
        sum += it["value"].as_i64().unwrap();
        ids += it["id"].as_i64().unwrap();
    }
    (sum, ids)
}

fn json_mixed() -> (i64, i64) {
    // The string values fail the plan iteration over mid loop.
    let text = r#"[{"v":1},{"v":"two"},{"v":3},{"v":"four"},{"v":5}]"#;
    let items: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
    let mut sum = 0;
    let mut misses = 0;
    for it in items {
        if let Some(n) = it["v"].as_i64() {
            sum += n;
        } else {
            misses += 1;
        }
    }
    (sum, misses)
}

fn checked_unwraps() -> (i64, i64) {
    let mut total: i64 = 0;
    let mut clamped = 0;
    for k in 0..2000i64 {
        total += k.checked_mul(3).unwrap();
        if let Some(shifted) = k.checked_add(i64::MAX - 1000) {
            clamped += shifted % 7;
        }
    }
    (total, clamped)
}

fn main() {
    for pair in word_counts() {
        println!("{} {}", pair.0, pair.1);
    }
    let (total, unique, width) = token_totals();
    println!("tokens total={total} unique={unique} width={width}");
    let (sum, ids) = json_sums();
    println!("json sum={sum} ids={ids}");
    let (sum, misses) = json_mixed();
    println!("mixed sum={sum} misses={misses}");
    let (total, clamped) = checked_unwraps();
    println!("checked total={total} clamped={clamped}");
}
