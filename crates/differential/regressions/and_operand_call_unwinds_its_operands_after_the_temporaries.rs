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

fn pair(_a: Trace, _b: Trace) -> bool {
    true
}

fn main() {
    let ready = opaque(1) == 1;
    let both = ready && pair(Trace(0), vec![Trace(17); 1][opaque(3)].clone());
    println!("{both}");
}
