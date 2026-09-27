use std::collections::HashMap;
use std::ops::AddAssign;

struct Meter(i64);

impl AddAssign<i64> for Meter {
    fn add_assign(&mut self, other: i64) {
        self.0 += other;
    }
}

struct Board {
    text: String,
    meter: Meter,
    count: usize,
}

fn note(label: &str) -> usize {
    println!("{label}");
    0
}

fn board<'a>(board: &'a mut Board, label: &str) -> &'a mut Board {
    println!("{label}");
    board
}

fn main() {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    *counts.entry(note("entry place")).or_insert(0) += note("entry value");
    let mut numbers = vec![0usize; 2];
    numbers[note("int place")] += note("int value");
    let mut meters = vec![Meter(0)];
    meters[note("meter place")] += note("meter value") as i64;
    let mut texts = vec![String::new()];
    texts[note("text place")] += &format!("{}", note("text value"));
    let mut b = Board {
        text: String::new(),
        meter: Meter(0),
        count: 0,
    };
    board(&mut b, "field text place").text += &format!("{}", note("field text value"));
    board(&mut b, "field meter place").meter += note("field meter value") as i64;
    board(&mut b, "field count place").count += note("field count value");
    println!("{} {} {} {}", counts[&0], numbers[0], meters[0].0, b.meter.0 + b.count as i64);
    println!("{:?} {:?}", texts, b.text);
}
