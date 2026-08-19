#!/usr/bin/env rust

//! A `sum` or `product` turbofish states the chain's type the same way a
//! `collect` turbofish does, and a later `checked_*` call answers an Option
//! of that width. The campaign found the width hint getting lost when a
//! `map` sat in the chain, so the None from an underflowing `checked_sub`
//! defaulted to an empty string instead of a zero of the summed type.

fn main() {
    let words = vec![String::from("rust")];
    let total = words.into_iter().map(|_w: String| 162u8).sum::<u8>();
    println!("summed: {total}");

    // 162 * 2 overflows u8, so the checked product is None and the default
    // must be a u8 zero. 162 - 254 underflows the same width.
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

    // 24 * 20000 overflows u16, the same default through the product spelling.
    println!(
        "product overflow default: {:?}",
        product.checked_mul(20000).unwrap_or_default()
    );

    // In range, the width rides along untouched.
    println!("in range: {:?}", total.checked_add(1).unwrap_or_default());
}
