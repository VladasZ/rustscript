#!/usr/bin/env rust

//! A `sum` or `product` turbofish states the chain's type. The width hint
//! once got lost when a `map` sat in the chain, so the default after a
//! `checked_sub` was an empty string instead of a zero.

fn main() {
    let words = vec![String::from("rust")];
    let total = words.into_iter().map(|_w: String| 162u8).sum::<u8>();
    println!("summed: {total}");

    // 162 * 2 overflows u8, so the default must be a u8 zero.
    println!(
        "overflow default: {:?}",
        total.checked_mul(2).unwrap_or_default()
    );
    println!("checked difference: {:?}", total.checked_sub(254));

    let product = vec![3u16, 5]
        .into_iter()
        .map(|x: u16| x + 1)
        .product::<u16>();
    println!("multiplied: {product}");

    // The same through `product`.
    println!(
        "product overflow default: {:?}",
        product.checked_mul(20000).unwrap_or_default()
    );

    // In range.
    println!("in range: {:?}", total.checked_add(1).unwrap_or_default());
}
