//! Generate the fixed input files the word_count, json, and regex cases read,
//! and the generated big_script case sources. Output is fully deterministic
//! from a seeded LCG, so every language reads identical bytes and the
//! correctness gate can compare stdout byte for byte.
//!
//! The base data files and the big_script sources are committed. The 10x
//! `data_big.*` files are too large for git, they are gitignored and `bench`
//! regenerates them on demand by running this binary.

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

/// The word_count and regex corpus. A vocabulary of short ascii tokens,
/// emitted with a skew toward low indices so the top words have clearly
/// separated counts.
fn words(tokens: u64) -> String {
    let vocab = 400u64;
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
    text
}

/// A json array of small records.
fn json(records: u64) -> String {
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
    json
}

/// The big_script case, a wide but trivial program in all three languages. It
/// measures how parse and compile time scale with source size, so the bodies
/// do next to nothing. Around a thousand lines each.
fn big_script(dir: &Path) {
    let funcs = 300u64;
    let mut rng = Lcg(0x5eed_5eed_5eed_5eed);
    let mut adds = Vec::new();
    for _ in 0..funcs {
        adds.push(rng.below(99991));
    }

    let mut rs = String::from("use std::time::Instant;\n\n");
    let mut ts = String::new();
    let mut py = String::from("import sys\nimport time\n\n");
    for (i, k) in adds.iter().enumerate() {
        let _ = write!(rs, "fn f{i:03}(x: i64) -> i64 {{\n    (x + {k}) % 99991\n}}\n\n");
        let _ = write!(ts, "function f{i:03}(x: number): number {{\n  return (x + {k}) % 99991;\n}}\n\n");
        let _ = write!(py, "def f{i:03}(x):\n    return (x + {k}) % 99991\n\n\n");
    }

    rs.push_str("fn main() {\n    let t = Instant::now();\n    let mut acc: i64 = 1;\n");
    ts.push_str("const t = performance.now();\nlet acc = 1;\n");
    py.push_str("t = time.perf_counter_ns()\nacc = 1\n");
    for i in 0..funcs {
        let _ = write!(rs, "    acc = f{i:03}(acc);\n");
        let _ = write!(ts, "acc = f{i:03}(acc);\n");
        let _ = write!(py, "acc = f{i:03}(acc)\n");
    }
    rs.push_str(
        r#"    let ns = t.elapsed().as_nanos();
    println!("acc = {acc}");
    eprintln!("COMPUTE_NS {ns}");
}
"#,
    );
    ts.push_str(
        r#"const ns = Math.round((performance.now() - t) * 1e6);
console.log(`acc = ${acc}`);
console.error(`COMPUTE_NS ${ns}`);
"#,
    );
    py.push_str(
        r#"ns = time.perf_counter_ns() - t
print(f"acc = {acc}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
"#,
    );

    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("case.rs"), rs).unwrap();
    std::fs::write(dir.join("case.ts"), ts).unwrap();
    std::fs::write(dir.join("case.py"), py).unwrap();
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let big = std::env::args().any(|a| a == "--big");

    if big {
        // The 10x tier inputs, gitignored, regenerated on demand.
        let text = words(2_500_000);
        std::fs::write(root.join("cases/word_count/data_big.txt"), &text).unwrap();
        let json = json(2_000_000);
        std::fs::write(root.join("cases/json/data_big.json"), &json).unwrap();
        println!("wrote {} bytes of text, {} bytes of json", text.len(), json.len());
        return;
    }

    let text = words(250_000);
    std::fs::write(root.join("cases/word_count/data.txt"), &text).unwrap();
    let json = json(200_000);
    std::fs::write(root.join("cases/json/data.json"), &json).unwrap();
    big_script(&root.join("cases/big_script"));
    println!("wrote {} bytes of text, {} bytes of json, big_script sources", text.len(), json.len());
}
