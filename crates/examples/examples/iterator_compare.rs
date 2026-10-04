#!/usr/bin/env rust

//! `cmp` and `partial_cmp` between 2 iterators go item by item, and a side that ends first is
//! the smaller one. A NaN makes `partial_cmp` answer `None`, on a float and on an iterator.

fn main() {
    let a = vec![1u8, 2, 3];
    let b = vec![1u8, 2, 4];
    println!("{:?}", a.iter().cmp(b.iter()));
    println!("{:?}", b.iter().cmp(a.iter()));
    println!("{:?}", a.iter().cmp(a.iter()));
    println!("{:?}", a.iter().cmp(a.iter().take(2)));
    println!("{:?}", a.iter().take(1).cmp(a.iter()));
    println!("{:?}", a.clone().into_iter().cmp(b.clone()));
    println!("{:?}", "abc".chars().cmp("abz".chars()));

    let with_nan = [1.0f64, f64::NAN];
    let plain = [1.0f64, 2.0];
    println!("{:?}", with_nan.iter().partial_cmp(plain.iter()));
    println!("{:?}", plain.iter().partial_cmp(plain.iter()));
    println!("{:?}", plain.iter().partial_cmp(with_nan.iter().take(1)));

    println!("{:?}", 1.0f64.partial_cmp(&2.0));
    println!("{:?}", 1.0f64.partial_cmp(&f64::NAN));
    println!("{:?}", f64::NAN.partial_cmp(&1.0));
    println!("{:?}", 1.0f32.partial_cmp(&f32::NAN));
    println!("{:?}", 2.5f32.partial_cmp(&2.5));
}
