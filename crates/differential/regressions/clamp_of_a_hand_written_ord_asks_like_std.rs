// A hand written `Ord` decides `max`, `min` and `clamp`, and its `cmp` prints, so the calls
// std makes are output. A `clamp` whose bounds are the wrong way round by that `Ord` panics.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rev(i32);

impl PartialOrd for Rev {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rev {
    fn cmp(&self, other: &Self) -> Ordering {
        println!("cmp {} {}", self.0, other.0);
        other.0.cmp(&self.0)
    }
}

fn main() {
    println!("{:?}", Rev(1).max(Rev(2)));
    println!("{:?}", std::cmp::min(Rev(1), Rev(2)));
    println!("{:?}", Rev(2).clamp(Rev(3), Rev(1)));
    println!("{:?}", Rev(2).clamp(Rev(1), Rev(3)));
}
