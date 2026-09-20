#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn pair() -> (u8, i64) {
    (0, 0)
}

fn main() {
    let mut first = Trace(1);
    let second = first;
    match pair() {
        (a, b) => {
            first = Trace(2);
        }
    }
    println!("{first:?} {second:?}");
}
