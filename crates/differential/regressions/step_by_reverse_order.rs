fn main() {
    let seen: Vec<i32> = (0..6)
        .map(|n| {
            println!("make {n}");
            n
        })
        .step_by(2)
        .rev()
        .collect();
    println!("{seen:?}");
}
