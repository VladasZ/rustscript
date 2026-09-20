#[derive(Debug, PartialEq)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn yes() -> bool {
    true
}

fn main() {
    let mut stack = vec![Trace(1), Trace(2)];
    while let Some(_) = stack.pop() {
        if yes() {
            continue;
        }
    }
    println!("after while let");
    for turn in 0..1i64 {
        let local = Trace(10 + turn);
        match Trace(20 + turn) == Trace(30 + turn) {
            false => {
                let inner = Trace(40 + turn);
                if yes() {
                    continue;
                }
                println!("unreachable {}", inner.0);
            }
            true => println!("equal {}", local.0),
        }
    }
    println!("end");
}
