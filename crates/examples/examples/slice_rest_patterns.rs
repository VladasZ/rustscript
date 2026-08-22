fn classify(v: &[i32]) -> String {
    match v {
        [] => "empty".to_string(),
        [only] => format!("one {only}"),
        [first, rest @ ..] if rest.len() > 3 => format!("{first} then {} more", rest.len()),
        [first, rest @ ..] => format!("{first} rest {rest:?}"),
    }
}

/// One element falls past the 2 ended arm.
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

    // A tail after the rest.
    let arr = [10, 20, 30, 40, 50];
    let [a, .., y, z] = arr;
    println!("{a} {y} {z}");
    let [head @ .., last] = arr;
    println!("{head:?} then {last}");
    let [first, mid @ .., end] = arr;
    println!("{first} {mid:?} {end}");

    // A vec derefs to a slice at the call.
    let grown: Vec<i32> = (1..=6).collect();
    println!("{}", classify(&grown));
    println!("{}", ends(&grown));
}
