use std::env;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let input: u64 = env::args().nth(1).context("missing input")?.parse()?;
    let value = input.wrapping_mul(48_271).wrapping_add(12_345) % 2_147_483_647;
    println!("{value}");
    Ok(())
}
