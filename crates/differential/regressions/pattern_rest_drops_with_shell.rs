//! The parts of a by value pattern that bind nothing, `_` in `Some((a, _))`, drop with the rest
//! of the scrutinee once the block ends, after the bindings. They used to leak, the bindings
//! shared the payload and dropping the shell would have dropped them twice. A guard reads the
//! bindings before the move, a `return` or `break` out of the block drops the shell too, and a
//! local that was partially moved drops its rest at its own scope end.

#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn pair() -> Option<(T, T)> {
    Some((T(1), T(2)))
}

fn if_let_return() {
    if let Some((a, _)) = pair() {
        println!("{}", a.0);
        return;
    }
    println!("not here");
}

fn let_else_diverge() {
    let Some((a, _)) = pair() else {
        println!("else");
        return;
    };
    println!("{}", a.0);
}

fn main() {
    if let Some((a, _)) = pair() {
        println!("{}", a.0);
    }
    println!("if let");
    match pair() {
        Some((a, _)) if a.0 > 5 => println!("big {}", a.0),
        Some((_, b)) => println!("small {}", b.0),
        None => println!("none"),
    }
    println!("guard");
    let kept = match pair() {
        Some((a, _)) => a,
        None => T(0),
    };
    println!("kept {}", kept.0);
    let mut v = vec![Some((T(3), T(4))), Some((T(5), T(6)))];
    while let Some(Some((a, _))) = v.pop() {
        println!("turn {}", a.0);
        break;
    }
    println!("while let");
    if_let_return();
    let_else_diverge();
    let p = pair();
    if let Some((a, _)) = p {
        println!("{}", a.0);
    }
    println!("end");
}
