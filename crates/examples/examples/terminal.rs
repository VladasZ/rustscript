#!/usr/bin/env rust

// The colored crate drops the color codes when output is not a terminal.

use colored::Colorize;
use std::io::{self, IsTerminal};

fn main() {
    let interactive = io::stdout().is_terminal();
    println!("stdout is a terminal: {interactive}");

    let styled = "status".green().bold().to_string();
    println!(
        "styled clean when piped: {}",
        styled == "status" || interactive
    );
}
