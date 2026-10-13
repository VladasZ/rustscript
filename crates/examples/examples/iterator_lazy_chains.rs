#!/usr/bin/env rust

// Adapters run item by item like in std. `flat_map` and `flatten` open an inner iterator only
// when the one before it is used up, a string `split` is an iterator and not a vec, `rposition`
// asks from the back, and what a chain still holds drops when the chain goes away.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn items(base: i64) -> Vec<T> {
    vec![T(base + 1), T(base + 2), T(base + 3), T(base + 4)]
}

fn flat() {
    println!("-- flat_map stops with take");
    {
        let out: Vec<T> = items(10)
            .into_iter()
            .flat_map(|x| vec![x.clone(), x])
            .take(3)
            .collect();
        println!("{out:?}");
    }

    println!("-- flat_map from the back");
    {
        let out: Vec<i64> = items(20)
            .into_iter()
            .flat_map(|x| vec![x.0, x.0 * 10])
            .rev()
            .take(3)
            .collect();
        println!("{out:?}");
    }

    println!("-- flatten of options");
    {
        let found: Vec<Option<T>> = items(30)
            .into_iter()
            .map(|x| if x.0 % 2 == 0 { Some(x) } else { None })
            .collect();
        let out: Vec<T> = found.into_iter().flatten().collect();
        println!("{out:?}");
    }

    println!("-- flat_map runs between the other closures");
    {
        let out: Vec<i64> = [1i64, 2]
            .into_iter()
            .inspect(|x| println!("outer {x}"))
            .flat_map(|x| vec![x, x + 10])
            .inspect(|x| println!("inner {x}"))
            .collect();
        println!("{out:?}");
    }
}

fn scans() {
    println!("-- scan keeps its state to the end");
    {
        let out: Vec<i64> = items(40)
            .into_iter()
            .scan(T(0), |state, x| {
                *state = T(state.0 + x.0);
                Some(state.0)
            })
            .collect();
        println!("{out:?}");
    }

    println!("-- scan state keeps its width");
    {
        let last = "abcdefghij"
            .chars()
            .scan(128u16, |state, _| {
                *state = state.wrapping_shl(1);
                Some(*state)
            })
            .last();
        println!("{last:?}");
        let zeros: Vec<u32> = "ab"
            .chars()
            .scan(255u8, |state, _| Some(state.leading_zeros()))
            .collect();
        println!("{zeros:?}");
    }
}

fn backs() {
    println!("-- rposition asks from the back");
    {
        let found = items(50).into_iter().rposition(|x| x.0 == 52);
        println!("{found:?}");
    }

    println!("-- unzip hands 2 owned vecs out");
    {
        let (left, right): (Vec<T>, Vec<T>) = items(60).into_iter().zip(items(65)).unzip();
        println!("{left:?} {right:?}");
    }

    println!("-- rev after a source with no back step");
    {
        let lines: Vec<String> = "a\nb\nc"
            .lines()
            .map(|line| format!("{line}!"))
            .rev()
            .inspect(|line| println!("line {line}"))
            .collect();
        println!("{lines:?}");
        let out: Vec<T> = items(70).into_iter().peekable().rev().take(2).collect();
        println!("{out:?}");
    }
}

fn splits() {
    println!("-- split is an iterator");
    let out: Vec<String> = "a,b,,c"
        .split(',')
        .map(|part| format!("<{part}>"))
        .inspect(|part| println!("part {part}"))
        .rev()
        .collect();
    println!("{out:?}");
    let mut parts = "x y z".split(' ');
    parts.next();
    println!("{:?}", parts.next());
    println!("{:?}", parts.collect::<Vec<&str>>());
    println!("{:?}", "a-b-c".rsplit('-').collect::<Vec<_>>());
    println!("{:?}", "a-b-c".splitn(2, '-').collect::<Vec<_>>());
    println!("{}", "abcabc".matches("bc").count());
    println!("{:?}", "héllo".char_indices().rev().collect::<Vec<_>>());
    for (index, part) in "q,w".split(',').enumerate() {
        println!("{index} {part}");
    }
    let sum: i32 = "1,2,3"
        .split(',')
        .map(|part| part.parse::<i32>().unwrap())
        .sum();
    println!("{sum}");
}

fn main() {
    flat();
    scans();
    backs();
    splits();
    println!("-- end");
}
