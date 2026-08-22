//! A chained `concat` keeps its element type. The second `concat` once fell
//! back to the string join, so an empty `Vec<Vec<T>>` printed as `""`. From
//! seed 241759.

fn main() {
    let one = Vec::<Vec<(f64, i16)>>::new().concat();
    println!("one: {one:?}");

    let two = Vec::<Vec<Vec<(f64, i16)>>>::new().concat().concat();
    println!("two: {two:?}");

    let three = Vec::<Vec<Vec<Vec<i32>>>>::new().concat().concat().concat();
    println!("three: {three:?}");

    // Real strings still join.
    let joined = Vec::<String>::new().concat();
    println!("joined: {joined:?}");

    let filled = [vec![vec![1i32, 2], vec![3]], vec![vec![4]]]
        .concat()
        .concat();
    println!("filled: {filled:?}");
}
