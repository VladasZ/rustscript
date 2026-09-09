// A consumed method receiver drops during the unwind when an argument panics before the call.
#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let empty: Vec<T> = Vec::new();
    println!("{:?}", Ok::<T, String>(T(0)).unwrap_or(empty[2].clone()));
}
