// A `saturating_pow` with a 4 billion exponent must square, not loop, or the run times out.
// Seed 20692105304.

fn diff_opaque_i64(value: i64) -> i64 {
    value
}

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    let v: usize = vec![0.0f64, 1.0].into_iter().map(|_| ((diff_opaque_i64(-1) as i32).saturating_pow((diff_opaque_u64(4294967294) as u32)) as usize)).sum();
    println!("{}", v);
    println!("{}", (diff_opaque_i64(-1) as i32).saturating_pow(4294967295));
    println!("{}", (diff_opaque_i64(2) as i32).saturating_pow(4294967295));
    println!("{}", (diff_opaque_i64(-2) as i32).saturating_pow(4294967295));
    println!("{}", (diff_opaque_i64(-2) as i32).saturating_pow(31));
    println!("{}", (diff_opaque_i64(-2) as i32).saturating_pow(32));
    println!("{}", (diff_opaque_i64(3) as u8).saturating_pow(6));
    println!("{}", (diff_opaque_i64(-1) as i8).wrapping_pow(4294967295));
    println!("{}", (diff_opaque_i64(3) as i8).wrapping_pow(4294967295));
    println!("{}", (diff_opaque_i64(3) as u64).wrapping_pow(4294967295));
    println!("{}", (diff_opaque_i64(7) as u64).checked_pow(4294967295).is_none());
    println!("{}", (diff_opaque_i64(1) as u64).checked_pow(4294967295).unwrap());
    println!("{}", (diff_opaque_i64(0) as u64).pow(4294967295));
    println!("{}", (diff_opaque_i64(-3) as i64).pow(39));
}
