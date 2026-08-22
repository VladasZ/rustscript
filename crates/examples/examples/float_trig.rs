//! Trig and total ordering on floats, in both widths. The f32 results go
//! through the real f32 path, so `{:?}` prints the short form instead of a
//! double rounded f64 value.

use std::cmp::Ordering;

fn main() {
    let angle: f64 = 0.75;
    println!("sin {:.12}", angle.sin());
    println!("cos {:.12}", angle.cos());
    println!("tan {:.12}", angle.tan());
    println!("asin {:.12}", angle.asin());
    println!("acos {:.12}", angle.acos());
    println!("atan {:.12}", angle.atan());
    println!("atan2 {:.12}", angle.atan2(2.0));
    println!("sinh {:.12}", angle.sinh());
    println!("cosh {:.12}", angle.cosh());
    println!("tanh {:.12}", angle.tanh());

    // a full turn lands back on the start
    let turn = std::f64::consts::TAU;
    println!("sin tau {:.12}", turn.sin());
    println!("cos tau {:.12}", turn.cos());

    // the f32 path, not the f64 core rounded down
    let small: f32 = 0.75;
    println!("f32 sin {:?}", small.sin());
    println!("f32 cos {:?}", small.cos());
    println!("f32 atan2 {:?}", small.atan2(2.0));

    // total_cmp orders NaN and both signed zeroes, where partial_cmp gives up
    let mut values = vec![0.5_f64, f64::NAN, -0.0, 0.0, -1.5, f64::INFINITY];
    values.sort_by(f64::total_cmp);
    for value in &values {
        println!("sorted {value:?}");
    }

    println!("cmp less {:?}", 1.0_f64.total_cmp(&2.0));
    println!("cmp equal {:?}", 2.0_f64.total_cmp(&2.0));
    println!("cmp greater {:?}", 3.0_f64.total_cmp(&2.0));
    println!("f32 cmp {:?}", 1.0_f32.total_cmp(&2.0));

    let zeroes = 0.0_f64.total_cmp(&-0.0);
    println!(
        "positive zero above negative {}",
        matches!(zeroes, Ordering::Greater)
    );

    // the float constants
    println!("pi {:.12}", std::f64::consts::PI);
    println!("tau {:.12}", std::f64::consts::TAU);
    println!("e {:.12}", std::f64::consts::E);
}
