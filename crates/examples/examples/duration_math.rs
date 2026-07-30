#!/usr/bin/env rust

// Duration addition and subtraction land on the checked std ops, so an
// accumulator built with `+=` matches compiled Rust in every accessor.

use std::time::Duration;

fn main() {
    let short = Duration::from_millis(1500);
    let long = Duration::from_millis(2600);
    let mut total = Duration::from_secs(0);
    total += short;
    total += long;
    println!("secs {}", total.as_secs());
    println!("millis {}", total.as_millis());
    println!("micros {}", total.as_micros());
    println!("subsec micros {}", total.subsec_micros());
    println!("float {}", total.as_secs_f64());
    let gap = long - short;
    println!("gap millis {}", gap.as_millis());
    println!("sum secs {}", (short + long).as_secs());
}
