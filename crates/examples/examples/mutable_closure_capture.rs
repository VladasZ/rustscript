fn main() {
    let input = [2_i64, -1, 4].to_vec();
    let mut total = 10_i64;
    let output: Vec<i64> = {
        let mut accumulate = |value: i64| {
            total = total.saturating_add(value);
            total
        };

        input
            .iter()
            .copied()
            .map(|value| accumulate(value.saturating_mul(2)))
            .collect()
    };

    println!("{total} {output:?}");
}
