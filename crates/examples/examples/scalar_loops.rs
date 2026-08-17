//! Loops the scalar loop plan specializes: int-only bodies over string
//! bytes and integer ranges. Each block also exercises one edge the plan
//! must keep identical to the generic path: width-tagged accumulators,
//! break and continue, inner while loops, unused loop values, and bodies
//! that fall back mid-loop because a register is not an integer.

fn main() {
    let text = String::from("the quick brown fox jumps over the lazy dog");

    let mut checksum: u64 = 0;
    for byte in text.bytes() {
        checksum = (checksum + u64::from(byte)) % 1_000_000_007;
    }
    println!("checksum {checksum}");

    let mut vowels: i64 = 0;
    let mut spaces: i64 = 0;
    for byte in text.bytes() {
        if byte == b' ' {
            spaces += 1;
            continue;
        }
        if byte == b'a' || byte == b'e' || byte == b'i' || byte == b'o' || byte == b'u' {
            vowels += 1;
        }
    }
    println!("vowels {vowels} spaces {spaces}");

    let mut first_z: i64 = -1;
    let mut index: i64 = 0;
    for byte in text.bytes() {
        if byte == b'z' {
            first_z = index;
            break;
        }
        index += 1;
    }
    println!("first z at {first_z}");

    let mut odd_sum: u32 = 0;
    for n in 1..=99 {
        if n % 2 == 1 {
            odd_sum += n as u32;
        }
    }
    println!("odd sum {odd_sum}");

    let mut collatz_total: u64 = 0;
    for start in 1..30 {
        let mut n = start as u64;
        while n != 1 {
            n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
            collatz_total += 1;
        }
    }
    println!("collatz steps {collatz_total}");

    let mut count: i64 = 0;
    for _ in 0..12345 {
        count += 1;
    }
    println!("count {count}");

    let mut empty_iterations: i64 = 0;
    for _ in 0..0 {
        empty_iterations += 1;
    }
    println!("empty {empty_iterations}");

    // A float in the body keeps the loop on the generic path, the plan
    // falls back on its first read and the result must not change.
    let mut mixed = 0.0f64;
    let mut shifted: i64 = 0;
    for n in 0..50 {
        mixed += 0.5;
        shifted = (shifted << 1) ^ n;
        shifted &= 0xFFFF;
    }
    println!("mixed {mixed} shifted {shifted}");

    let mut last = 0u8;
    for byte in text.bytes() {
        last = byte;
    }
    println!("last {last}");

    // The plan runs the first ninety items, then the branch reads a float
    // register it cannot handle, and the generic path must resume with the
    // accumulator and the iterator exactly where the plan left them.
    let fraction = 2.5f64;
    let mut caught: i64 = 0;
    let mut total: i64 = 0;
    for n in 0..100 {
        total += n;
        if n == 90 {
            caught = fraction as i64;
        }
    }
    println!("total {total} caught {caught}");
}
