#!/usr/bin/env rust

// A tuple pattern in a closure parameter list next to plain parameters, in folds, direct calls,
// comparators and `fn` parameters.

fn span((lo, hi): (i32, i32), pad: i32) -> i32 {
    hi - lo + pad
}

fn main() {
    let values = [4, -2, 9, 7];
    let (lo, hi) = values
        .iter()
        .fold((i32::MAX, i32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    println!("{lo} {hi} {}", span((lo, hi), 1));

    let (sum, count) = values.iter().fold((0, 0), |(s, c), x| (s + x, c + 1));
    println!("{sum} {count} {}", sum / count);

    let add = |(a, b): (i32, i32), c: i32| a + b + c;
    let swap = |c: i32, (a, b): (i32, i32)| (b - c, a + c);
    println!("{} {:?}", add((1, 2), 3), swap(3, (1, 2)));

    let pairs = vec![("b", 2), ("a", 2), ("c", 1)];
    let mut sorted = pairs.clone();
    sorted.sort_by(|&(n1, v1), &(n2, v2)| v2.cmp(&v1).then(n1.cmp(n2)));
    println!("{sorted:?}");

    let joined: Vec<String> = pairs
        .iter()
        .enumerate()
        .map(|(i, (name, v))| format!("{i}:{name}={v}"))
        .collect();
    println!("{joined:?}");

    let nested = [((1, 2), 3), ((4, 5), 6)];
    let total: i32 = nested.iter().map(|((a, b), c)| a * b + c).sum();
    println!("{total}");

    let mut acc = 0;
    let mut bump = |(a, b): (i32, i32)| {
        acc += a * b;
        acc
    };
    bump((2, 3));
    bump((4, 5));
    println!("{acc}");

    let stats = values
        .iter()
        .fold((0, 0, 0), |(neg, zero, pos), &x| match x {
            v if v < 0 => (neg + 1, zero, pos),
            0 => (neg, zero + 1, pos),
            _ => (neg, zero, pos + 1),
        });
    println!("{stats:?}");
}
