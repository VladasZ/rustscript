//! A `break value` that reads a field moves the field out, like a `let` or `return` value. The
//! interpreter used to clone the field and share its inner value with the loop temporary that then
//! dropped, which emptied the moved value. The trace showed `Trace` with no field and the final
//! drop panicked with `no field 0`.

#[derive(Debug, Clone)]
struct Trace(i64);
impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
#[derive(Debug, Clone)]
struct Holder {
    field: Trace,
}
fn main() {
    let mut holder = Holder { field: Trace(1) };
    // the field moves out of the clone through the break, the clone keeps nothing to drop twice
    let moved = loop {
        break holder.clone().field;
    };
    holder.field = moved;
    println!("holder: {holder:?}");

    // a tuple field through the same machinery
    let pair = (Trace(2), Trace(3));
    let second = loop {
        break pair.clone().1;
    };
    println!("second: {second:?}");
}
