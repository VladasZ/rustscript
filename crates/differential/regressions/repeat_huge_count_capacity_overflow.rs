// A count past `i64::MAX` used to saturate to `isize::MAX` on the way into `repeat`, which is an
// allocation failure instead of the `capacity overflow` panic. Seed 20690117324.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    println!("{}", String::from("").repeat(diff_opaque_u64(16700004588372137953) as usize).len());
    println!("{}", Vec::<u8>::new().repeat(diff_opaque_u64(16700004588372137953) as usize).len());
    println!("{}", String::from("0").repeat(diff_opaque_u64(16700004588372137953) as usize).matches(" 5 ").count());
}
