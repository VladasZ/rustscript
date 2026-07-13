#!/usr/bin/env rust

// Terminal helpers: detect whether output is a terminal, and style text. The
// colored crate drops the color codes when output is not a terminal, so piped
// output stays clean.

use colored::Colorize;
use std::io::{self, IsTerminal};

fn main() {
    let interactive = io::stdout().is_terminal();
    println!("stdout is a terminal: {}", interactive);

    let styled = "status".green().bold().to_string();
    // When not a tty the styled text equals the plain text.
    println!(
        "styled clean when piped: {}",
        styled == "status" || interactive
    );
}
