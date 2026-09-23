#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let got = 'b: {
        if got_early() {
            break 'b Trace(9);
        }
        Trace(1)
    };
    println!("got: {got:?}");
    let tail = 'c: { Trace(2) };
    println!("tail: {tail:?}");
}

fn got_early() -> bool {
    true
}
