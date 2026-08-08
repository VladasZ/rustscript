#!/usr/bin/env rust

//! f32 methods compute in real f32 and their results stay f32, so `{:?}`
//! prints the f32 shortest form, `3.4028235e38` for `f32::MAX`, never the
//! f64 image `3.4028234663852886e38`. NaN handling in `min` and `max` and
//! the rounding family match the compiled binary bit for bit.

fn main() {
    let largest: f32 = f32::MAX;
    println!("max:        {largest:?}");
    println!("min nan:    {:?}", largest.min(f32::NAN));
    println!("max nan:    {:?}", f32::NAN.max(largest));
    println!("min:        {:?}", 1.5f32.min(2.5f32));
    println!("max:        {:?}", 1.5f32.max(2.5f32));
    println!("clamp:      {:?}", 3.3f32.clamp(0.5f32, 2.0f32));

    println!("abs:        {:?}", (-1.5f32).abs());
    println!("sqrt:       {:?}", 1.5f32.sqrt());
    println!("powi:       {:?}", 1.3f32.powi(3));
    println!("powf:       {:?}", 1.3f32.powf(0.5f32));

    println!("floor:      {:?}", 2.7f32.floor());
    println!("ceil:       {:?}", 2.2f32.ceil());
    println!("round:      {:?}", 2.5f32.round());
    println!("trunc:      {:?}", (-2.7f32).trunc());

    println!("sign pos:   {:?}", (-0.0f32).is_sign_positive());
    println!("cmp:        {:?}", 1.5f32.partial_cmp(&2.5f32));

    // Chained results stay f32 too.
    let tiny: f32 = f32::MIN_POSITIVE;
    println!("tiny:       {tiny:?}");
    println!("chained:    {:?}", tiny.max(0.1f32).sqrt().min(largest));
}
