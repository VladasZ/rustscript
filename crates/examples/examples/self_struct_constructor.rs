struct Counter {
    value: i64,
}

impl Counter {
    fn new(value: i64) -> Self {
        Self { value }
    }

    fn increased(mut self, delta: i64) -> Self {
        self.value = self.value.saturating_add(delta);
        self
    }

    fn total(&self) -> i64 {
        self.value
    }
}

fn main() {
    let counter = Counter::new(4i64).increased(3i64);
    println!("{}", counter.total());
}
