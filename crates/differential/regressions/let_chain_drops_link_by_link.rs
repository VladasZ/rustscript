// A let chain drops each link's binding, then that link's scrutinee temporary, last link first,
// at the end of the body and on a `break`, `continue` or `return` out of it.
// Found by seed 20721119697.

#[derive(Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn early() -> i64 {
    let _keep = T(0);
    if let Some(x) = vec![T(1), T(2)].first().cloned() {
        return x.0;
    }
    9
}

fn main() {
    println!("early {}", early());
    if let Some(x) = vec![T(3), T(4)].first().cloned()
        && x.0 == 3
        && let Some(y) = vec![T(5), T(6)].last().cloned()
    {
        let z = T(9);
        println!("chain {} {} {}", x.0, y.0, z.0);
    }
    for _ in 0..1 {
        if let Some(x) = vec![T(10), T(11)].first().cloned()
            && let Some(y) = vec![T(12), T(13)].last().cloned()
        {
            println!("continue {} {}", x.0, y.0);
            continue;
        }
    }
    loop {
        if let Some(x) = vec![T(14), T(15)].first().cloned() {
            println!("break {}", x.0);
            break;
        }
    }
}
