#!/usr/bin/env rust

// The handler is not triggered, the program only confirms it installed.

fn main() -> anyhow::Result<()> {
    ctrlc::set_handler(|| {
        println!("interrupted");
    })?;
    println!("handler installed");
    Ok(())
}
