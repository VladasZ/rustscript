// The default after `checked_add` is a number, so the divide by zero message must show. Seed
// 20673100883.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    println!("lang_9: {:?}", ((diff_opaque_u64(52) as u8) / (diff_opaque_u64(255) as u8).checked_add((diff_opaque_u64(255) as u8)).unwrap_or_default()));
    println!("{:?}", ());
}
