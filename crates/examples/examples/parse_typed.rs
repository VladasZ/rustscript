#!/usr/bin/env rust

//! `parse` honors its turbofish, `"300".parse::<u8>()` must be an `Err`. Otherwise a script
//! validating with `parse().is_err()` accepts garbage.

fn main() {
    println!("u8 in range:    {:?}", "255".parse::<u8>());
    println!("u8 over range:  {:?}", "300".parse::<u8>().is_err());
    println!("u8 negative:    {:?}", "-1".parse::<u8>().is_err());
    // "-0" is an error for an unsigned target even though it fits
    println!("u64 minus zero: {:?}", "-0".parse::<u64>().is_err());
    println!("i8 min:         {:?}", "-128".parse::<i8>());
    println!("i8 under range: {:?}", "-129".parse::<i8>().is_err());
    println!("u32 leading nl: {:?}", "007".parse::<u32>());
    println!("i32 plus sign:  {:?}", "+7".parse::<i32>());

    // whitespace is an error
    println!("padded:         {:?}", " 5 ".parse::<i64>().is_err());
    println!("trailing space: {:?}", "5 ".parse::<i64>().is_err());

    println!("f64 decimal:    {:?}", "1.5".parse::<f64>());
    println!("u32 decimal:    {:?}", "1.5".parse::<u32>().is_err());
    println!("f32 decimal:    {:?}", "0.25".parse::<f32>());

    println!("bool true:      {:?}", "true".parse::<bool>());
    println!("bool one:       {:?}", "1".parse::<bool>().is_err());
    println!("char single:    {:?}", "a".parse::<char>());
    println!("char many:      {:?}", "NaN".parse::<char>().is_err());

    // the annotation drives it without a turbofish
    let annotated: u8 = "42".parse().unwrap();
    println!("annotated u8:   {annotated}");

    let count = "12".parse::<u16>().unwrap_or(0);
    println!("unwrap_or:      {count}");
}
