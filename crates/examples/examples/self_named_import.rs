#!/usr/bin/env rust

// `use which::which` must not resolve the bare call to the import again.

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
