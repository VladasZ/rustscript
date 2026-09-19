//! A bare `ref mut` binding over a scalar local borrows the local itself in every binding
//! form, `let`, `if let`, `while let` and a `match` arm, so the write lands in the local.

fn opaque(value: i64) -> i64 {
    value
}

fn main() {
    let mut n = opaque(5);
    if let ref mut m = n {
        *m += 1;
    }
    println!("{n}");

    let mut total = opaque(1);
    let ref mut acc = total;
    *acc += 41;
    println!("{total}");

    let mut text = String::from("ab");
    let ref mut t = text;
    t.push('c');
    println!("{text}");

    let mut rounds = opaque(0);
    while let ref mut r = rounds {
        *r += 1;
        if *r == 3 {
            break;
        }
    }
    println!("{rounds}");

    let mut flag = false;
    match flag {
        ref mut f => *f = true,
    }
    println!("{flag}");
}
