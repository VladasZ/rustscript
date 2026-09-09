#!/usr/bin/env rust

// `?` takes its receiver. The outer `Ok` is a temporary of the statement, and its drop must
// not empty the payload it already handed out.

struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn inner() -> Result<bool, String> {
    Ok::<Result<bool, String>, String>(Ok(false))?
}
fn main() {
    println!("{:?}", inner());
    println!("{}", T(1).0);
}
