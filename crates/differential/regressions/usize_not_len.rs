//! A length is a real `usize`. `!vec.len()` once answered a small negative
//! i64 instead of a huge unsigned. From seed 20675317577.

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
