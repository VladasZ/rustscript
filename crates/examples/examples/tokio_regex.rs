// The regex family and Duration accessors on the parallel engine that
// `#[tokio::main]` selects, backed by the same shared cores as the fast
// engine: match spans, named captures, replace, split, and the full
// Duration accessor set.

use std::time::Duration;

use regex::Regex;

#[tokio::main]
async fn main() {
    let re = Regex::new(r"(?<word>[a-z]+)-(\d+)").unwrap();
    let text = "alpha-1 beta-22 gamma-333";

    println!("pattern: {}", re.as_str());
    println!("is_match: {}", re.is_match(text));

    if let Some(found) = re.find(text) {
        println!(
            "find: {} [{}..{}]",
            found.as_str(),
            found.start(),
            found.end()
        );
    }

    if let Some(caps) = re.captures("beta-22") {
        println!("len: {}", caps.len());
        println!("word: {}", caps.name("word").unwrap().as_str());
        println!("num: {}", caps.get(2).unwrap().as_str());
        println!("missing: {}", caps.name("nope").is_none());
    }

    let words: Vec<&str> = re.find_iter(text).map(|m| m.as_str()).collect();
    println!("all: {words:?}");

    println!("replaced: {}", re.replace_all(text, "x"));
    let pieces: Vec<&str> = re.split(text).collect();
    println!("pieces: {pieces:?}");

    let d = Duration::from_millis(1500);
    println!(
        "dur: {} {} {} {} {} {}",
        d.as_secs(),
        d.as_millis(),
        d.as_micros(),
        d.subsec_millis(),
        d.as_secs_f64(),
        d.is_zero()
    );
}
