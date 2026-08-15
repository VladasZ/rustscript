// A fn param declared u8 must compute at the u8 bound inside the body, so
// the add here panics with the overflow message exactly like debug Rust.
// Before the param retag, the body computed wide and printed 260.

fn add_ten(v: u8) -> u8 {
    v + 10
}

fn main() {
    println!("start:{}", add_ten(1));
    println!("overflow:{}", add_ten(250));
}
