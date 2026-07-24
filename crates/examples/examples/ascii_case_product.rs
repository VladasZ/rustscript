// The ascii case methods leave non-ascii characters alone, unlike their
// unicode cousins, and iterator product multiplies like sum adds.

fn main() {
    let text = "λ Mixed CASE 123";
    println!("{}", text.to_uppercase());
    println!("{}", text.to_lowercase());
    println!("{}", text.to_ascii_uppercase());
    println!("{}", text.to_ascii_lowercase());
    println!("{}", "ΛΛΛ abc".to_ascii_lowercase());

    let mut values = vec![2i64, 3, 5];
    values.push(7);
    println!("{}", values.iter().product::<i64>());
    println!("{}", values.iter().sum::<i64>());
    let empty: Vec<i64> = Vec::new();
    println!("{}", empty.iter().product::<i64>());
    let mut floats = vec![0.5f64, 4.0];
    floats.push(2.5);
    println!("{}", floats.iter().product::<f64>());
}
