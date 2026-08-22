// From seeds 20673106218 and 20673000410. The generic dispatch once flattened
// Option arguments to i64, so `unwrap_or` dropped a u8's width. The struct
// is load bearing, it forces that dispatch path.

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

    // The u8 keeps its width through `unwrap_or`.
    let counted: u32 = None::<u8>.unwrap_or((diff_opaque_u64(255) as u8)).rotate_right((diff_opaque_u64(1) as u32)).count_zeros();
    println!("count_zeros: {:?}", counted);

    // A u64 past `i64::MAX` survives `unwrap_or`.
    println!("large: {:?}", None::<usize>.unwrap_or((diff_opaque_u64(14422477213308566380) as usize)));
    println!("swapped: {:?}", None::<usize>.unwrap_or((diff_opaque_u64(14422477213308566380) as usize)).swap_bytes());

    // Counts are u32, so `!` wraps at 32 bits.
    println!("not_ones: {:?}", (!((diff_opaque_u64(0) as u16) >> (diff_opaque_u64(3) as u32)).count_ones()));

    // `then_some` keeps the width tag. From seeds 20673218374 and
    // 20673012633.
    println!("then_some_not: {:?}", true.then_some((!None::<u64>.unwrap_or((diff_opaque_u64(0) as u64)))));
    println!("then_some_wide: {:?}", true.then_some(((diff_opaque_i64(-2147483648) as i32) as usize)));

    // `len()` is a usize, so the subtraction floors at 0. From seed
    // 20673115610.
    println!("len_sub: {:?}", String::from("1").len().saturating_sub(((diff_opaque_u64(16690579655298140432) as usize).max((diff_opaque_u64(0) as usize)) | String::from("1.5").chars().count())));
}
