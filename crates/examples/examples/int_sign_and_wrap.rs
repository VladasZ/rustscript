#!/usr/bin/env rust

//! `unsigned_abs`, the saturating and wrapping divisions, the logarithm with a base, `to_be`,
//! `to_le` and the casts between the signed and the unsigned type of 1 size.

fn main() {
    // the answer is the unsigned type of the same size, `!x` shows the width
    println!("{} {}", (-5i32).unsigned_abs(), !(-5i8).unsigned_abs());
    println!("{} {}", i8::MIN.unsigned_abs(), i64::MIN.unsigned_abs());
    println!("{} {}", i128::MIN.unsigned_abs(), 7isize.unsigned_abs());

    println!(
        "{} {} {}",
        i8::MIN.saturating_abs(),
        (-3i16).saturating_abs(),
        i128::MIN.saturating_abs()
    );
    println!(
        "{} {} {}",
        i8::MIN.saturating_neg(),
        7i32.saturating_neg(),
        i64::MIN.saturating_neg()
    );
    println!(
        "{} {} {}",
        i8::MIN.saturating_div(-1),
        9u8.saturating_div(2),
        (-9i64).saturating_div(2)
    );
    println!(
        "{} {} {}",
        i8::MIN.wrapping_div(-1),
        100u8.wrapping_div(7),
        (-7i32).wrapping_div(2)
    );
    println!("{} {}", i8::MIN.wrapping_rem(-1), (-7i32).wrapping_rem(2));
    println!(
        "{} {}",
        i8::MIN.wrapping_div_euclid(-1),
        (-7i32).wrapping_div_euclid(2)
    );
    println!(
        "{} {}",
        i8::MIN.wrapping_rem_euclid(-1),
        (-7i32).wrapping_rem_euclid(2)
    );
    println!(
        "{:?} {:?} {:?}",
        (-7i32).checked_div_euclid(2),
        7u8.checked_div_euclid(0),
        i8::MIN.checked_div_euclid(-1)
    );

    println!(
        "{:?} {:?} {:?}",
        1000u32.checked_ilog10(),
        0u8.checked_ilog10(),
        (-4i64).checked_ilog10()
    );
    println!("{} {} {}", 81u32.ilog(3), 5i64.ilog(5), u64::MAX.ilog(7));
    println!(
        "{:?} {:?} {:?}",
        81u32.checked_ilog(3),
        81u32.checked_ilog(1),
        0u8.checked_ilog(2)
    );
    // a count is a `u32` whatever the receiver is
    println!("{:?}", 100u8.checked_ilog(10).map(|count| !count));
    println!("{}", !200u8.ilog(3));

    println!("{:#06x} {:#06x}", 0x1234u16.to_be(), 0x1234u16.to_le());
    println!(
        "{:#x} {:#x}",
        (-2i32).to_be(),
        0x0102_0304_0506_0708u64.to_be()
    );

    println!("{} {}", (-1i8).cast_unsigned(), !(-1i32).cast_unsigned());
    println!("{} {}", 200u8.cast_signed(), u64::MAX.cast_signed());
    println!("{} {}", 5usize.cast_signed(), (-5isize).cast_unsigned());
    println!("{} {}", u128::MAX.cast_signed(), (-1i128).cast_unsigned());

    let wide = 1u128 << 100;
    println!("{} {:?}", wide.ilog(4), wide.checked_ilog10());
    println!("{} {}", i128::MIN.wrapping_div(-1), 7u128.saturating_div(2));
    println!("{} {}", 3i128.abs_diff(-4), wide.to_le() == wide);
    println!(
        "{:?} {}",
        (-7i128).checked_div_euclid(2),
        (-7i128).wrapping_rem_euclid(2)
    );
}
