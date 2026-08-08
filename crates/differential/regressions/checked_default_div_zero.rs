// Reduced from differential seed 20673100883. `checked_add` overflows to
// None, `unwrap_or_default` must build a zero of the receiver's width, and
// the division then panics with the compiled binary's exact divide-by-zero
// message. The default used to come out as an empty string, which panicked
// with "expected a number" instead.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    println!("lang_9: {:?}", ((diff_opaque_u64(52) as u8) / (diff_opaque_u64(255) as u8).checked_add((diff_opaque_u64(255) as u8)).unwrap_or_default()));
    println!("{:?}", ());
}
