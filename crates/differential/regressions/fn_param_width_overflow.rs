// A u8 param must overflow at the u8 bound. Before the param retag the body
// printed 260.

fn add_ten(v: u8) -> u8 {
    v + 10
}

fn main() {
    println!("start:{}", add_ten(1));
    println!("overflow:{}", add_ten(250));
}
