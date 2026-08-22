//! A chained `concat` keeps its element type, so an empty `Vec<Vec<T>>` prints as `[]` and not
//! `""`. Seed 241759.

fn main() {
    let one = Vec::<Vec<(f64, i16)>>::new().concat();
    println!("one: {one:?}");

    let two = Vec::<Vec<Vec<(f64, i16)>>>::new().concat().concat();
    println!("two: {two:?}");

    let three = Vec::<Vec<Vec<Vec<i32>>>>::new().concat().concat().concat();
    println!("three: {three:?}");

    // real strings still join
    let joined = Vec::<String>::new().concat();
    println!("joined: {joined:?}");

    let filled = [vec![vec![1i32, 2], vec![3]], vec![vec![4]]]
        .concat()
        .concat();
    println!("filled: {filled:?}");
}
