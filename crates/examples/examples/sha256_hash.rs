#!/usr/bin/env rust

// SHA-256 three ways: the one-shot `Sha256::digest`, the incremental
// `new` + `update` + `finalize`, and the chained `chain_update`. Hashing the
// same bytes every way must produce the same digest, which the last line
// asserts by printing whether all three agree.

use sha2::{Digest, Sha256};

fn main() {
    let one_shot = hex::encode(Sha256::digest("the quick brown fox"));

    let mut hasher = Sha256::new();
    hasher.update("the quick ");
    hasher.update("brown fox");
    let incremental = hex::encode(hasher.finalize());

    let chained = hex::encode(Sha256::new().chain_update("the quick brown fox").finalize());

    println!("one_shot    {one_shot}");
    println!("incremental {incremental}");
    println!("chained     {chained}");
    println!(
        "all equal   {}",
        one_shot == incremental && incremental == chained
    );
}
