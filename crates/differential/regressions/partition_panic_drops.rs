#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn three() -> usize {
    3
}

fn main() {
    let parts: (Vec<Trace>, Vec<Trace>) = vec![Trace(1), Trace(2), Trace(3), Trace(4), Trace(5)]
        .into_iter()
        .partition(|item| if item.0 == 4 { Vec::<bool>::new()[three()] } else { item.0 % 2 == 0 });
    println!("{parts:?}");
}
