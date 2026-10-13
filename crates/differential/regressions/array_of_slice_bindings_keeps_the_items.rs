// A binding that holds a slice or a reference goes into an array, a vec, a tuple or a
// `Some` as a reference, so the holder drops nothing of the owner.

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let v = vec![T(7), T(8), T(9)];
    let w = vec![T(3), T(4), T(5)];
    {
        let (a, b) = w.split_at(1);
        let joined = [b, a].concat();
        println!("{:?}", joined);
    }
    println!("a");
    {
        let (a, b) = w.split_at(1);
        let parts = vec![b, a];
        println!("{:?}", parts);
        let pair = (a, b);
        println!("{:?}", pair);
        let some = Some(a);
        println!("{:?}", some);
    }
    println!("b");
    {
        let a = v.as_slice();
        let b = &w[1..];
        let joined = [a, b].concat();
        println!("{:?}", joined);
        let all = [a, b];
        println!("{}", all.len());
    }
    println!("c");
    {
        let first = &v[0];
        let second = v.first().unwrap();
        let both = [first, second];
        println!("{:?}", both);
        let mut made = vec![first, second];
        made.push(first);
        println!("{:?}", made);
    }
    println!("end {:?} {:?}", v, w);
}
