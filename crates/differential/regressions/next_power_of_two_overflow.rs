//! Past the top power `next_power_of_two` panics in debug Rust and the checked form returns
//! `None`. The panicking half cannot live in the equivalence suite.

fn main() {
    println!("{:?}", 200u8.checked_next_power_of_two());
    println!("{:?}", u64::MAX.checked_next_power_of_two());
    println!("{}", 200u8.next_power_of_two());
}
