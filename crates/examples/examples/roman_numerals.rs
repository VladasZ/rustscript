#!/usr/bin/env rust

fn to_roman(mut n: i64) -> String {
    let table = vec![
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, sym) in table {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    out
}

fn main() {
    for n in vec![4, 9, 14, 40, 90, 2024] {
        println!("{n} = {}", to_roman(n));
    }
}
