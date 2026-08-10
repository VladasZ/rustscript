//! A container or string length is a real `usize`, so `!` and `%` on it run
//! in the unsigned width. The campaign found `!vec.len()` answering a small
//! negative i64 where compiled Rust answers a huge unsigned, which then
//! steered a `%` to a wrong remainder two expressions later. From seed
//! 20675317577.

fn opaque(v: u64) -> u64 {
    v
}

fn main() {
    let values = vec![1.5f32, 2.5f32];
    let masked = !values.len();
    println!("{masked}");
    let folded = (opaque(6141963268824554873) as usize) % masked;
    println!("{folded}");
    println!("{}", !folded);
    println!("{}", !"abc".len());
    println!("{}", !"hello".chars().count());
}
