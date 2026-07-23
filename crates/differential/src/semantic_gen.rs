use rand::RngExt;
use rand::rngs::StdRng;

use crate::semantic::SemanticCase;

pub fn generate_semantic_cases(rng: &mut StdRng) -> Vec<SemanticCase> {
    let count = rng.random_range(1..=3);
    let mut kinds = [0, 1, 2, 3];
    for index in 0..count {
        let other = rng.random_range(index..kinds.len());
        kinds.swap(index, other);
    }
    kinds[..count]
        .iter()
        .enumerate()
        .map(|(id, kind)| generate_case(id, *kind, rng))
        .collect()
}

fn generate_case(id: usize, kind: usize, rng: &mut StdRng) -> SemanticCase {
    match kind {
        0 => {
            let values = random_values(rng, 8);
            SemanticCase::BorrowedVector {
                id,
                start: rng.random_range(0..=values.len()),
                take: rng.random_range(0..=values.len().saturating_add(2)),
                values,
                delta: rng.random_range(-5..=5),
            }
        }
        1 => SemanticCase::OwnedRecord {
            id,
            label: word(rng).to_string(),
            values: random_values(rng, 7),
            extra: rng.random_range(-20..=20),
        },
        2 => SemanticCase::ResultFlow {
            id,
            left: result_word(rng).to_string(),
            right: result_word(rng).to_string(),
            reject_negative: rng.random_bool(0.5),
            fallback: rng.random_range(-20..=20),
        },
        _ => SemanticCase::IteratorControl {
            id,
            values: random_values(rng, 10),
            parity: rng.random_range(0..=1),
            limit: rng.random_range(0..=8),
            skip_negative: rng.random_bool(0.5),
        },
    }
}

fn random_values(rng: &mut StdRng, maximum: usize) -> Vec<i64> {
    let count = rng.random_range(0..=maximum);
    (0..count).map(|_| rng.random_range(-30..=30)).collect()
}

fn word(rng: &mut StdRng) -> &'static str {
    const WORDS: &[&str] = &["", "record", "rust script", "λ", "line\nbreak"];
    WORDS[rng.random_range(0..WORDS.len())]
}

fn result_word(rng: &mut StdRng) -> &'static str {
    const WORDS: &[&str] = &["negative", "zero", "one", "large", "unknown", " one ", ""];
    WORDS[rng.random_range(0..WORDS.len())]
}
