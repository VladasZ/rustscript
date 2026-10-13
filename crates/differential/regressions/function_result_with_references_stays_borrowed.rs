// A script function that returns an `Option<&T>`, a `Vec<&T>` or a tuple with a `&T` hands
// out storage the caller does not own, so the caller drops nothing of it. A `cloned` of
// such an `Option` is a value of its own.

#[derive(Debug, PartialEq, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct Bag {
    items: Vec<T>,
}

impl Bag {
    fn find(&self, n: i64) -> Option<&T> {
        for item in &self.items {
            if item.0 == n {
                return Some(item);
            }
        }
        None
    }

    fn big(&self) -> Vec<&T> {
        self.items.iter().filter(|t| t.0 > 4).collect()
    }
}

fn first_big(v: &[T]) -> Option<&T> {
    v.iter().find(|t| t.0 > 4)
}

fn pick(v: &[T]) -> Vec<&T> {
    v.iter().filter(|t| t.0 > 4).collect()
}

fn pair(v: &[T]) -> (&T, usize) {
    (&v[0], v.len())
}

fn parse(text: &str) -> Result<T, &str> {
    if text == "3" { Ok(T(3)) } else { Err(text) }
}

fn main() {
    let v = vec![T(5), T(4), T(7)];
    {
        let b = first_big(&v);
        println!("{:?}", b);
    }
    println!("a");
    {
        let p = pick(&v);
        println!("{:?}", p);
    }
    println!("b");
    {
        let p = pair(&v);
        println!("{:?}", p);
    }
    println!("c");
    let bag = Bag { items: vec![T(1), T(6)] };
    {
        let found = bag.find(6);
        println!("{:?}", found);
        let big = bag.big();
        println!("{:?}", big);
    }
    println!("d");
    {
        let found = bag.find(1).cloned();
        println!("{:?}", found);
    }
    println!("e");
    {
        let parsed = parse("3");
        println!("{:?} {:?}", parsed, parse("x"));
    }
    println!("end {:?}", v);
}
