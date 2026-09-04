//! An iterator held in a binding hands out items of its own, so what `next` and `last` return
//! drops, and so does a `map` whose closure builds a fresh value over a lending receiver.
//! They used to count as lent and never dropped.

#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn next_from_binding() {
    let mut it = vec![T(1), T(2)].into_iter();
    let x = it.next();
    println!("{}", x.is_some());
}

fn while_let_next() {
    let mut it = vec![T(1), T(2), T(3)].into_iter();
    while let Some(x) = it.next() {
        println!("turn {}", x.0);
    }
    println!("end");
}

fn map_over_lending() {
    let v = vec![T(1), T(2)];
    let x = v.iter().map(|x| x.clone()).last();
    println!("{}", x.is_some());
    let m = v.iter().map(|x| T(x.0 + 10));
    let y = m.last();
    println!("{}", y.is_some());
    for z in v.iter().map(|x| x.clone()) {
        println!("turn {}", z.0);
    }
    println!("end");
}

fn main() {
    next_from_binding();
    while_let_next();
    map_over_lending();
}
