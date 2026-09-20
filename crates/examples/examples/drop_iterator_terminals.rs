#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let parts: (Vec<Trace>, Vec<Trace>) = vec![Trace(1), Trace(2), Trace(3)]
        .into_iter()
        .partition(|item| item.0 % 2 == 0);
    println!("{parts:?}");
    let maximum = vec![Trace(4), Trace(5), Trace(6)]
        .into_iter()
        .max_by_key(|item| item.0 % 2);
    println!("{maximum:?}");
    let minimum = vec![Trace(7), Trace(8), Trace(9)]
        .into_iter()
        .min_by_key(|item| item.0 % 2);
    println!("{minimum:?}");
    let borrowed = [Trace(10), Trace(11)];
    println!("{:?}", borrowed.iter().max_by_key(|item| item.0));
    println!("{:?}", borrowed.iter().min_by_key(|item| item.0));
    let refs: (Vec<&Trace>, Vec<&Trace>) = borrowed.iter().partition(|item| item.0 % 2 == 0);
    println!("{refs:?}");
}
