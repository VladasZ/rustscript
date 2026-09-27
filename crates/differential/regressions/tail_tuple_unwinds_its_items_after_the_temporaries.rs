struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

impl Clone for Trace {
    fn clone(&self) -> Trace {
        Trace(self.0)
    }
}

fn opaque(value: usize) -> usize {
    value
}

fn pair(first: Trace) -> (Trace, Trace) {
    (first, vec![Trace(17); 1][opaque(3)].clone())
}

fn main() {
    let made = pair(Trace(0));
    println!("{}", made.0.0);
}
