//! A narrow `fold` init keeps its width, so `leading_zeros` counts 32 bits and not 64.

fn opaque(v: i64) -> i64 {
    v
}

fn main() {
    let init = opaque(502028173) as i32;
    let empty = Vec::<u16>::new()
        .into_iter()
        .map(|_x: u16| opaque(0) as i32)
        .fold(init, |acc, _x| acc);
    println!("{}", empty.leading_zeros() & opaque(119447032) as u32);
    let walked = vec![1u16, 2u16]
        .into_iter()
        .fold(init, |acc, _x| acc);
    println!("{}", walked.leading_zeros());
}
