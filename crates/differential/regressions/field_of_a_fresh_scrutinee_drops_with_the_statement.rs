// A match on a non copy field of a fresh value, `(make()).f`, takes the field out of the
// temporary. What no arm binds drops with the statement, after the arm and before the rest of
// the temporary.
// Found by seed 20715002989.

#[derive(Debug)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

struct S {
    a: Option<(T, T)>,
    b: T,
}

fn make(n: i64) -> S {
    S {
        a: Some((T(n), T(n + 1))),
        b: T(n + 2),
    }
}

fn main() {
    let x = match (make(1)).a {
        Some((p, _)) => p.0,
        None => 0,
    };
    println!("x {x}");
    let y = match make(10).a {
        None => 1,
        _ => 2,
    };
    println!("y {y}");
    if let Some((_, q)) = make(20).a {
        println!("q {:?}", q);
    }
    println!("end");
}
