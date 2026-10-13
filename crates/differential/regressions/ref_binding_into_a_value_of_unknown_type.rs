// `Some(ref mut r)` over a local the inference could not type, here the result of
// `VecDeque::front`, borrows the local, so a write through `r` lands in it.

use std::collections::VecDeque;

fn main() {
    let queue = VecDeque::from(vec![3usize, 4]);
    let mut front = queue.front().copied();
    if let Some(ref mut r) = front {
        *r += 1usize;
        println!("front {r}");
    }
    match front {
        Some(ref mut r) if *r > 3usize => *r += 10usize,
        Some(ref mut r) => *r = 0usize,
        None => {}
    }
    println!("{front:?}");
    let mut back = queue.back().copied();
    while let Some(ref mut r) = back {
        *r += 5usize;
        if *r > 12usize {
            break;
        }
    }
    println!("{back:?}");
    let mut pair = (queue.front().copied(), 1u8);
    if let (Some(ref mut r), _) = pair {
        *r += 7usize;
    }
    println!("{pair:?}");
}
