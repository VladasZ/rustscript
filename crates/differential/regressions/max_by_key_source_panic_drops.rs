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
    let parts = vec![Trace(1), Trace(2), Trace(3), Trace(4), Trace(5)]
        .into_iter()
        .map(|item| {
            println!("source {}", item.0);
            if item.0 == 4 { println!("{}", Vec::<bool>::new()[three()]); }
            item
        })
        .max_by_key(|item| item.0);
    println!("{parts:?}");
}
