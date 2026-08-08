// From differential seeds 20673106218 and 20673000410. A user method in the
// program routes every method call through the generic dispatch, which used
// to flatten Option arguments to plain i64: `unwrap_or` then dropped a u8's
// width so `count_zeros` counted 64 bits, and a u64 past `i64::MAX`
// saturated. The struct is load bearing, it forces that dispatch path.

struct GeneratedRecord0 {
    values: Vec<i64>,
}

impl GeneratedRecord0 {
    fn score(&self) -> i64 {
        self.values.iter().copied().fold(0i64, |total, value| total.saturating_add(value))
    }
}

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn diff_opaque_i64(value: i64) -> i64 {
    value
}

fn main() {
    let record = GeneratedRecord0 { values: vec![1i64, 2i64] };
    println!("score: {}", record.score());

    // The u8 keeps its width through `unwrap_or`, so the bit counts see 8
    // bits, not 64.
    let counted: u32 = None::<u8>.unwrap_or((diff_opaque_u64(255) as u8)).rotate_right((diff_opaque_u64(1) as u32)).count_zeros();
    println!("count_zeros: {:?}", counted);

    // A u64 past `i64::MAX` survives `unwrap_or` at its real value.
    println!("large: {:?}", None::<usize>.unwrap_or((diff_opaque_u64(14422477213308566380) as usize)));
    println!("swapped: {:?}", None::<usize>.unwrap_or((diff_opaque_u64(14422477213308566380) as usize)).swap_bytes());

    // The counting family answers u32, so `!` wraps at 32 bits.
    println!("not_ones: {:?}", (!((diff_opaque_u64(0) as u16) >> (diff_opaque_u64(3) as u32)).count_ones()));

    // `then_some` on a bool hands its argument through with the width tag
    // intact, so a u64 past `i64::MAX` and a sign-extended negative survive.
    // From seeds 20673218374 and 20673012633.
    println!("then_some_not: {:?}", true.then_some((!None::<u64>.unwrap_or((diff_opaque_u64(0) as u64)))));
    println!("then_some_wide: {:?}", true.then_some(((diff_opaque_i64(-2147483648) as i32) as usize)));

    // An untagged `len()` receiver takes the width its argument states, so
    // the usize subtraction floors at 0 instead of saturating at `i64::MIN`.
    // From seed 20673115610.
    println!("len_sub: {:?}", String::from("1").len().saturating_sub(((diff_opaque_u64(16690579655298140432) as usize).max((diff_opaque_u64(0) as usize)) | String::from("1.5").chars().count())));
}
