#!/usr/bin/env rust

//! Integer methods answer in the receiver's real width. These all used to run
//! on an i64 image, so saturation happened at i64's bounds and a u64 past
//! `i64::MAX` was clamped before the method ever saw it.

fn main() {
    let small: u8 = 200;
    println!("u8 saturating add: {}", small.saturating_add(100));
    println!("u8 saturating mul: {}", small.saturating_mul(2));
    println!("u8 wrapping add:   {}", small.wrapping_add(100));
    println!("u8 checked add:    {:?}", small.checked_add(100));
    println!("u8 checked add ok: {:?}", small.checked_add(1));

    let signed: i8 = -100;
    println!("i8 saturating sub: {}", signed.saturating_sub(100));
    println!("i8 abs:            {}", signed.abs());
    println!("i8 signum:         {}", signed.signum());
    println!("i8 wrapping mul:   {}", signed.wrapping_mul(3));

    // A u64 past i64::MAX keeps its full value through every method.
    let huge: u64 = u64::MAX;
    println!("u64 max:           {}", huge.max(12345));
    println!("u64 min:           {}", huge.min(12345));
    println!("u64 saturating:    {}", huge.saturating_add(1));
    println!("u64 checked add:   {:?}", huge.checked_add(1));
    println!("u64 leading zeros: {}", huge.leading_zeros());

    let bits: u16 = 0x1234;
    println!("u16 count ones:    {}", bits.count_ones());
    println!("u16 count zeros:   {}", bits.count_zeros());
    println!("u16 swap bytes:    {}", bits.swap_bytes());
    println!("u16 rotate left:   {}", bits.rotate_left(4));
    println!("u16 rotate right:  {}", bits.rotate_right(4));
    println!("u16 reverse bits:  {}", bits.reverse_bits());
    println!("u16 trailing:      {}", bits.trailing_zeros());

    let value: i32 = -17;
    println!("i32 div euclid:    {}", value.div_euclid(5));
    println!("i32 rem euclid:    {}", value.rem_euclid(5));
    // MIN % -1 overflows, so the checked remainder answers None for it.
    println!("i32 checked rem:   {:?}", i32::MIN.checked_rem(-1));
    println!("i64 checked rem:   {:?}", i64::MIN.checked_rem(-1));
    println!("i32 clamp:         {}", value.clamp(-10, 10));
    println!("i32 pow:           {}", 3i32.pow(4));
    println!("i32 isqrt:         {}", 17i32.isqrt());

    // Only zero is a multiple of zero, which is an answer rather than a panic.
    println!("zero multiple:     {}", 0u32.is_multiple_of(0));
    println!("five multiple:     {}", 5u32.is_multiple_of(0));
    println!("six multiple:      {}", 6u32.is_multiple_of(3));

    // The f64 method surface, including the sign and class tests.
    let float: f64 = -2.75;
    println!("f64 fract:         {:?}", float.fract());
    println!("f64 signum:        {:?}", float.signum());
    println!("f64 recip:         {:?}", float.recip());
    println!("f64 mul add:       {:?}", float.mul_add(2.0, 0.5));
    println!("f64 is nan:        {}", float.is_nan());
    println!("f64 nan is nan:    {}", f64::NAN.is_nan());
    println!("f64 is finite:     {}", float.is_finite());
    println!("f64 inf finite:    {}", f64::INFINITY.is_finite());
    println!("f64 is infinite:   {}", f64::NEG_INFINITY.is_infinite());
    println!("f64 sign negative: {}", float.is_sign_negative());
    println!("f64 zero negative: {}", (-0.0f64).is_sign_negative());

    // Float sums start from -0.0 in std, so the sign of a negative zero
    // survives the fold.
    let zeros = [-0.0f64, -0.0f64];
    println!(
        "negative zero sum: {:?}",
        zeros.iter().copied().sum::<f64>()
    );
    let mixed = [-0.0f64, 2.5f64];
    println!(
        "mixed sum:         {:?}",
        mixed.iter().copied().sum::<f64>()
    );

    // An empty sequence has no element to say which kind of sum this is, so
    // the turbofish is what tells an empty float sum from an empty integer one.
    println!(
        "empty f64 sum:     {:?}",
        Vec::<f64>::new().iter().copied().sum::<f64>()
    );
    println!(
        "empty f32 sum:     {:?}",
        Vec::<f32>::new().iter().copied().sum::<f32>()
    );
    println!(
        "empty i32 sum:     {:?}",
        Vec::<i32>::new().iter().copied().sum::<i32>()
    );
}
