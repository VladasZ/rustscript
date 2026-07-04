#!/usr/bin/env rustscript

fn main() {
    let log = "INFO start\nERROR disk full\nINFO ok\nERROR panic\nDEBUG trace";
    let errors: Vec<String> = log
        .lines()
        .filter(|line| line.contains("ERROR"))
        .map(|line| line.to_string())
        .collect();
    println!("{} errors found", errors.len());
    for e in errors {
        println!("{e}");
    }
}
