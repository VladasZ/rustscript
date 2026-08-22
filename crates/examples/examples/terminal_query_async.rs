#!/usr/bin/env rust

// The `#[tokio::main]` copy of `terminal_query`.

use crossterm::terminal::size;
use terminal_light::luma;

#[tokio::main]
async fn main() {
    match size() {
        Ok((cols, rows)) => println!("size known: {}", cols > 0 && rows > 0),
        Err(_) => println!("size unavailable"),
    }
    let skin = match luma() {
        Ok(luma) if luma > 0.6 => "light",
        Ok(_) => "dark",
        Err(_) => "unknown",
    };
    println!("skin {skin}");
}
