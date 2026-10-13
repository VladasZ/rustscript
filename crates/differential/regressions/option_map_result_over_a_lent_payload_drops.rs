// The value a `map` closure builds from a lent payload is the caller's own. It drops with
// its statement or its binding, also when the receiver is a temporary vec.

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let v = vec![T(7), T(8), T(9)];
    println!("{:?}", vec![T(1), T(2)].split_first().map(|(a, b)| (a.clone(), b.to_vec())));
    println!("{:?}", vec![T(1), T(2)].get(0..1).map(|c| c.to_vec()));
    println!("{:?}", vec![T(1), T(2)].as_array::<2>().map(|c| c.to_vec()));
    println!("{:?}", vec![T(1), T(2)].first().map(|t| t.clone()));
    println!("{:?}", vec![T(1), T(2)].last().cloned());
    println!("{:?}", vec![T(1), T(2)].iter().max_by_key(|t| t.0).map(|t| vec![t.clone()]));
    println!("{:?}", vec![T(1), T(2)].first().and_then(|t| Some(T(t.0 + 10))));
    println!("between");
    {
        let a = v.get(0..2).map(|c| c.to_vec()).unwrap_or_default();
        println!("{:?}", a);
    }
    println!("a");
    {
        let a = v.first().map(|t| t.clone());
        println!("{:?}", a);
        let b = v.last().map(|t| vec![t.clone()]).unwrap_or_default();
        println!("{:?}", b);
        let c = v.split_last().map(|(t, rest)| (t.clone(), rest.len()));
        println!("{:?}", c);
        let d = v.first().map(|t| t);
        println!("{:?}", d);
    }
    println!("end {:?}", v);
}
