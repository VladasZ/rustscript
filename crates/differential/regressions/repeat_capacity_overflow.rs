//! `repeat` past the allocation limit must panic with `capacity overflow` and exit 101, not die
//! in the allocator with exit 1. Seeds 20686107380 and 20686204456.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    println!("small: {}", String::from("ab").repeat(3));
    println!("vec: {:?}", vec![1i16, 2].repeat(2));
    // an empty receiver repeats to nothing
    println!(
        "empty: {:?}",
        Vec::<i16>::new().repeat(diff_opaque_u64(11528012806172059478) as usize)
    );
    let wide = vec![0i16].repeat(diff_opaque_u64(11528012806172059478) as usize);
    println!("{}", wide.len());
}
