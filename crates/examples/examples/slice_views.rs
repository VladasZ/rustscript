#!/usr/bin/env rust

// Slice shaped views of a vec: `as_slice` patterns, `windows`, `chunks`,
// `repeat`, `swap`, and `nth` on a lazy iterator.

fn describe(values: &[i32]) -> String {
    match values {
        [] => String::from("empty"),
        [one] => format!("one {one}"),
        [first, .., last] => format!("{first} to {last}"),
    }
}

fn main() {
    let v = vec![1, 2, 3, 4, 5];
    println!("{}", describe(v.as_slice()));
    println!("{}", describe(vec![7].as_slice()));
    println!("{}", describe(Vec::<i32>::new().as_slice()));
    match v.as_slice() {
        [head, rest @ ..] => println!("{head} then {rest:?}"),
        [] => println!("nothing"),
    }
    let sums: Vec<i32> = v.windows(2).map(|w| w[0] + w[1]).collect();
    println!("{sums:?} {}", v.windows(3).count());
    let lens: Vec<usize> = v.chunks(2).map(<[i32]>::len).collect();
    println!("{lens:?}");
    println!("{:?}", v.repeat(2));
    let mut w = v.clone();
    w.swap(0, 4);
    println!("{w:?}");
    println!(
        "{:?} {:?}",
        v.iter().copied().nth(3),
        v.iter().copied().nth(9)
    );
    println!("{:?}", v.into_iter().map(|x| x * 10).nth(1));
}
