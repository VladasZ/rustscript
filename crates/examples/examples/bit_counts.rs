#!/usr/bin/env rust

//! The bit counting family answers from the receiver's real width, and every
//! count is a u32 exactly as in compiled Rust, so `!count` wraps at 32 bits
//! instead of computing in i64.

fn main() {
    let byte: u8 = 0b1111_0111;
    println!("ones:            {}", byte.count_ones());
    println!("zeros:           {}", byte.count_zeros());
    println!("leading zeros:   {}", byte.leading_zeros());
    println!("trailing zeros:  {}", byte.trailing_zeros());

    let full: u8 = u8::MAX;
    println!("full zeros:      {}", full.count_zeros());
    println!("rotated zeros:   {}", full.rotate_right(1).count_zeros());

    let word: u16 = 0;
    println!("not ones:        {}", !(word >> 3).count_ones());
    println!("not zeros:       {:?}", !word.count_zeros());

    let wide: u64 = u64::MAX - 1;
    println!("wide ones:       {}", wide.count_ones());
    println!("signed ones:     {}", (-1i32).count_ones());

    // A count is a u32 value, so it casts and compares as one.
    println!("as u64:          {}", u64::from(byte.count_ones()));
    println!("compare:         {}", byte.count_ones() == 7);
}
