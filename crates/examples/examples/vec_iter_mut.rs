fn adjust(values: &mut [i64], delta: i64) {
    for value in values.iter_mut() {
        *value = (*value).saturating_add(delta);
    }
}

fn main() {
    let mut values = vec![-2i64, 0i64, 3i64, 8i64];
    adjust(&mut values, 2i64);
    for value in values.iter_mut().skip(1).take(2) {
        *value += 3i64;
    }
    println!("{values:?}");
}
