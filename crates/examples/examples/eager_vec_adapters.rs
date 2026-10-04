#!/usr/bin/env rust

//! An adapter the interpreter runs eagerly hands a vec on. `chain`, `zip`, `step_by`, `scan`,
//! `map_while`, `min_by`, `max_by`, `cycle` and `unzip` still work on what comes after it.

fn main() {
    let values = [5, 3, 8, 1];
    let ends: Vec<&i32> = values.first().into_iter().chain(values.last()).collect();
    println!("{ends:?}");
    let spread = values.iter().flat_map(|value| [*value, *value + 1]);
    println!("{:?}", spread.clone().step_by(3).collect::<Vec<_>>());
    println!(
        "{:?}",
        spread.clone().zip("abc".chars()).collect::<Vec<_>>()
    );
    println!(
        "{:?}",
        spread
            .clone()
            .max_by(|left, right| (left % 4).cmp(&(right % 4)))
    );
    println!(
        "{:?}",
        spread
            .clone()
            .min_by(|left, right| (left % 4).cmp(&(right % 4)))
    );
    println!(
        "{:?}",
        spread
            .clone()
            .scan(0, |total, value| {
                *total += value;
                Some(*total)
            })
            .collect::<Vec<_>>()
    );
    println!(
        "{:?}",
        spread
            .clone()
            .map_while(|value| (value != 8).then_some(value * 2))
            .collect::<Vec<_>>()
    );
    println!(
        "{:?}",
        spread.clone().cycle().skip(6).take(4).collect::<Vec<_>>()
    );
    let (small, big): (Vec<i32>, Vec<i64>) =
        spread.map(|value| (value, i64::from(value) * 100)).unzip();
    println!("{small:?} {big:?}");
}
