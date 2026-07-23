fn adjust(values: &mut [i64], delta: i64) {
    for value in values.iter_mut() {
        *value = (*value).saturating_add(delta);
    }
}

#[tokio::main]
async fn main() {
    let mut values = vec![1i64, 2i64, 3i64];
    adjust(&mut values, 4i64);
    println!("{values:?}");
}
