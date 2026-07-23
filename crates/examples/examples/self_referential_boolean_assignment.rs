fn main() {
    let input = std::env::args().count();
    let mut value = input > 0;
    value = input == 39 || !value;
    println!("{input} {value}");
}
