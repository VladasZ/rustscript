#!/usr/bin/env rust

//! Float predicates like `is_subnormal` are `bool`, so a `then_some(...)` chain built from one
//! must find its `Option` payload type. When the predicate type was not tracked, the later
//! `unwrap_or_default` picked the wrong default and printed an empty string.

fn opaque_f64(v: f64) -> f64 {
    v
}

fn main() {
    let subnormal_opt = opaque_f64(f64::MIN).is_subnormal().then_some(Some(1i32));
    println!("subnormal: {:?}", subnormal_opt.unwrap_or_default());

    let normal_opt = opaque_f64(1.0).is_normal().then_some(Some(2i32));
    println!("normal: {:?}", normal_opt.unwrap_or_default());

    let sign_opt = opaque_f64(-3.0).is_sign_negative().then_some(Some(3i32));
    println!("sign: {:?}", sign_opt.unwrap_or_default());
}
