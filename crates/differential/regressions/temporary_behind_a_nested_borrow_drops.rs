// `&&T(4)` makes a temporary behind 2 borrows. It drops when its statement ends.

#[derive(Debug, PartialEq)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let v = vec![T(5), T(4)];
    let held = vec![&v[0], &v[1]];
    println!("{}", v.contains(&T(8)));
    println!("{}", held.contains(&&T(4)));
    println!("{}", held.contains(&&T(9)));
    println!("end");
}
