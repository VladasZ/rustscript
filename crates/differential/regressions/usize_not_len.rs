//! A length is a real `usize`, so `!vec.len()` is a huge unsigned and not a small negative i64.
//! Seed 20675317577.

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
