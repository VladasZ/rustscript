#!/usr/bin/env rust

// Ask the terminal how big it is and how bright its background is. Both
// answers depend on the window the program runs in, so this prints only what
// it decided, never the raw reading.

use crossterm::terminal::size;
use terminal_light::luma;

fn main() {
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
