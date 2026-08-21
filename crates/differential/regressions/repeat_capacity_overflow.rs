//! `repeat` past the allocation limit is a script panic, not an interpreter
//! crash. The campaign found the interpreter dying inside its own allocator
//! and exiting 1 where the real binary panics with `capacity overflow` and
//! exits 101. From seeds 20686107380 and 20686204456.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    println!("small: {}", String::from("ab").repeat(3));
    println!("vec: {:?}", vec![1i16, 2].repeat(2));
    // An empty receiver repeats to nothing however large the count is.
    println!(
        "empty: {:?}",
        Vec::<i16>::new().repeat(diff_opaque_u64(11528012806172059478) as usize)
    );
    let wide = vec![0i16].repeat(diff_opaque_u64(11528012806172059478) as usize);
    println!("{}", wide.len());
}
