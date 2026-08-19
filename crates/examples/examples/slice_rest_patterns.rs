//! Slice patterns with a `..` rest: bare and named, at the front, middle,
//! and back, on slices, arrays, and `&mut` scrutinees.

fn classify(v: &[i32]) -> String {
    match v {
        [] => "empty".to_string(),
        [only] => format!("one {only}"),
        [first, rest @ ..] if rest.len() > 3 => format!("{first} then {} more", rest.len()),
        [first, rest @ ..] => format!("{first} rest {rest:?}"),
    }
}

/// A rest arm still needs its written elements present, so one element
/// falls past the two-ended arm.
fn ends(v: &[i32]) -> String {
    match v {
        [x, .., y] => format!("pair {x} {y}"),
        [x, ..] => format!("single {x}"),
        [] => "none".to_string(),
    }
}

fn main() {
    println!("{}", classify(&[]));
    println!("{}", classify(&[7]));
    println!("{}", classify(&[1, 2]));
    println!("{}", classify(&[1, 2, 3, 4, 5]));

    println!("{}", ends(&[]));
    println!("{}", ends(&[9]));
    println!("{}", ends(&[3, 4]));
    println!("{}", ends(&[3, 5, 8, 4]));

    // A tail after the rest binds the last elements in written order.
    let arr = [10, 20, 30, 40, 50];
    let [a, .., y, z] = arr;
    println!("{a} {y} {z}");
    let [head @ .., last] = arr;
    println!("{head:?} then {last}");
    let [first, mid @ .., end] = arr;
    println!("{first} {mid:?} {end}");

    // A vec dereferences to a slice at the call, same as compiled Rust.
    let grown: Vec<i32> = (1..=6).collect();
    println!("{}", classify(&grown));
    println!("{}", ends(&grown));
}
