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
    let parts = vec![Trace(1), Trace(2), Trace(3)]
        .into_iter()
        .min_by_key(|item| if item.0 == 1 { 1 } else { Vec::<i64>::new()[three()] });
    println!("{parts:?}");
}
