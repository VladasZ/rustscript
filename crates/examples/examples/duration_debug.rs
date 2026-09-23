#!/usr/bin/env rust


use std::time::Duration;

#[derive(Debug)]
struct Timing {
    took: Duration,
}

fn main() {
    let cases = [
        Duration::from_secs(0),
        Duration::from_nanos(1),
        Duration::from_nanos(999),
        Duration::from_nanos(1_000),
        Duration::from_nanos(1_500),
        Duration::from_micros(999_999),
        Duration::from_millis(1),
        Duration::from_nanos(416_055_800),
        Duration::from_millis(1500),
        Duration::from_secs(90),
        Duration::new(1, 999_999_999),
        Duration::from_nanos(123_456),
    ];
    for d in cases {
        println!(
            "[{d:?}] [{d:.0?}] [{d:.1?}] [{d:.2?}] [{d:.12?}] [{d:10?}] [{d:>10.1?}] [{d:*^12?}] [{d:+?}] [{d:#?}]"
        );
    }
    println!(
        "{:?}",
        vec![Duration::from_millis(5), Duration::from_secs(2)]
    );
    println!("{:?}", Some(Duration::from_micros(3)));
    let timing = Timing {
        took: Duration::from_millis(12),
    };
    println!("{timing:?} {}", timing.took.as_millis());
    println!("{timing:#?}");
}
