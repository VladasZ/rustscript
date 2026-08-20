#!/usr/bin/env rust

// An import that repeats its crate name, `use which::which`. The bare call
// must resolve to the crate function and not to the import again, and the
// written out `which::which` after the import still starts at the crate.
// This also holds inside a module with its own copy of the import.

use glob::glob;
use which::which;

mod probe {
    use which::which;

    pub fn has_cargo() -> bool {
        which("cargo").is_ok()
    }
}

fn main() {
    println!("bare: {}", which("cargo").is_ok());
    println!("qualified: {}", which::which("cargo").is_ok());
    println!("missing: {}", which("definitely-not-a-real-tool").is_ok());
    println!("module: {}", probe::has_cargo());
    let pattern = std::env::temp_dir()
        .join("rustscript-self-named-import-none-*")
        .display()
        .to_string();
    let matched = glob(&pattern).map(Iterator::count);
    println!("glob: {matched:?}");
}
