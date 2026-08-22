#!/usr/bin/env rust

// All 3 ways must agree on the digest.

use hex::encode;
use sha2::{Digest, Sha256};

fn main() {
    let one_shot = encode(Sha256::digest("the quick brown fox"));

    let mut hasher = Sha256::new();
    hasher.update("the quick ");
    hasher.update("brown fox");
    let incremental = encode(hasher.finalize());

    let chained = encode(Sha256::new().chain_update("the quick brown fox").finalize());

    println!("one_shot    {one_shot}");
    println!("incremental {incremental}");
    println!("chained     {chained}");
    println!(
        "all equal   {}",
        one_shot == incremental && incremental == chained
    );
}
