//! Generate the fixed input files the word_count and json cases read. Output is
//! fully deterministic from a seeded LCG, so every language reads identical
//! bytes and the correctness gate can compare stdout byte for byte. Run once and
//! commit the results.

use std::fmt::Write as _;
use std::path::Path;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    /// A value in `0..bound`, taken from the high bits which mix best.
    fn below(&mut self, bound: u64) -> u64 {
        (self.next() >> 33) % bound
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Word list. A vocabulary of short ascii tokens, emitted with a skew toward
    // low indices so the top words have clearly separated counts.
    let vocab = 400u64;
    let tokens = 250_000u64;
    let mut rng = Lcg(0x1234_5678_9abc_def0);
    let mut text = String::new();
    for i in 0..tokens {
        let a = rng.below(vocab);
        let b = rng.below(vocab);
        let idx = a * b / vocab;
        let _ = write!(text, "w{idx:04}");
        if (i + 1) % 12 == 0 {
            text.push('\n');
        } else {
            text.push(' ');
        }
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    std::fs::write(root.join("cases/word_count/data.txt"), &text).unwrap();

    // Json array of small records.
    let records = 200_000u64;
    let mut rng = Lcg(0x0fed_cba9_8765_4321);
    let mut json = String::from("[");
    for i in 0..records {
        if i > 0 {
            json.push(',');
        }
        let value = rng.below(1000);
        let _ = write!(json, "{{\"id\":{i},\"value\":{value}}}");
    }
    json.push(']');
    std::fs::write(root.join("cases/json/data.json"), &json).unwrap();

    println!("wrote {} bytes of text, {} bytes of json", text.len(), json.len());
}
