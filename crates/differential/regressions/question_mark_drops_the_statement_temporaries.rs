// A `?` that returns early drops the temporaries its statement already made, the left operand
// of a comparison included, like a `return`.
// Found by seed 91000009174.

#[derive(Debug, PartialEq, PartialOrd)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make(n: i64) -> T {
    T(n)
}

fn in_condition() -> Option<i32> {
    if T(1) > (None::<T>?) {
        return Some(1);
    }
    None
}

fn in_let() -> Option<i32> {
    let _keep = T(2);
    let x = make(3) == (None::<T>?);
    println!("{x}");
    None
}

fn in_tuple() -> Option<i32> {
    let x = (T(4), None::<T>?);
    println!("{:?}", x);
    None
}

fn main() {
    println!("{:?}", in_condition());
    println!("{:?}", in_let());
    println!("{:?}", in_tuple());
}
