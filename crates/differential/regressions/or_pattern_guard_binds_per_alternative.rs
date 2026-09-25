// A guard runs once per alternative of the arm's or-patterns, nested ones too, in the order
// rustc tries them, and each alternative binds on its own. Found by seed 20721207157.

struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    match ((1, 2), (3, 4)) {
        ((a, _) | (_, a), (b, _) | (_, b)) if {
            println!("try {a} {b}");
            a == 2 && b == 3
        } =>
        {
            println!("won {a} {b}")
        }
        _ => println!("none"),
    }
    match (1, 2) {
        (x, _) | (_, x) if x == 2 => println!("x is {x}"),
        _ => println!("none"),
    }
    match Some((5, 6)) {
        Some((v, _) | (_, v)) | Some((_, v)) if {
            println!("some {v}");
            false
        } => {}
        _ => println!("done"),
    }
    let mut n = 0;
    match 1 {
        1 | 1 if {
            n += 1;
            n == 2
        } => println!("second try wins {n}"),
        _ => println!("no {n}"),
    }
    println!("{}", matches!((1, 2), (x, _) | (_, x) if x == 2));
    match (T(1), T(2)) {
        (x, _) | (_, x) if x.0 == 2 => println!("took {}", x.0),
        _ => println!("none"),
    }
    println!("end");
}
