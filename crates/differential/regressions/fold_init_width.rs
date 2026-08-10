//! `fold` hands its initial value through to the closure and the result, so
//! a narrow integer init keeps its width. The campaign found the method
//! dispatch flattening fold's arguments to plain i64, which made
//! `leading_zeros` on the folded value count 64 bits instead of 32.

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
