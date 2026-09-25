// A pattern with a `ref` binding next to a by value one borrows a local and moves the by value
// parts out of it. The moved parts drop with the bindings, the rest drops with the local, and
// the local stays readable where it was not moved. Match, `if let`, `let` and `let else`.
// Found by seed 20719016550.

#[derive(Debug)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Debug)]
struct S {
    a: T,
    t: T,
}

fn main() {
    let v = S { a: T(10), t: T(1) };
    match v {
        S { a: ref a, t } => println!("arm {:?} {:?}", a, t),
    }
    println!("after {:?}", v.a);
    let w = (T(20), T(2));
    let n = match (w) {
        (ref x, y) => x.0 + y.0,
    };
    println!("n {n} {:?}", w.0);
    let o = Some((T(30), T(3)));
    if let Some((ref p, q)) = o {
        println!("if {:?} {:?}", p, q);
    }
    println!("after if");
    let u = (T(40), T(4));
    {
        let (ref x, y) = u;
        println!("let {:?} {:?}", x, y);
    }
    println!("after let {:?}", u.0);
    let e = Some((T(50), T(5)));
    {
        let Some((ref p, q)) = e else { return };
        println!("else {:?} {:?}", p, q);
    }
    println!("end");
}
