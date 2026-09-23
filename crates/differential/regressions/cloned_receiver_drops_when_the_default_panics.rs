#[derive(Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let v = vec![T(1); 3].get(1).cloned().unwrap_or(vec![T(2), T(3)][4].clone());
    println!("{}", v.0);
}
