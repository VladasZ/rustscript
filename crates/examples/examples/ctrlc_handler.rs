#!/usr/bin/env rust

// Register a Ctrl-C handler. The handler is not triggered here, so the program
// just confirms it installed and exits.

fn main() -> anyhow::Result<()> {
    ctrlc::set_handler(|| {
        println!("interrupted");
    })?;
    println!("handler installed");
    Ok(())
}
