#!/usr/bin/env rust

// `let r = &make()` keeps the temporary alive to the end of the enclosing scope, `&mut make()`
// and `&make().field` too, while a borrow of a local extends nothing.

struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make(n: i64) -> Trace {
    Trace(n)
}

fn main() {
    {
        let a = &Trace(1);
        let b = &make(2);
        println!("in {} {}", a.0, b.0);
    }
    println!("after the block");

    let field = &make(3).0;
    let wide = &mut Trace(4);
    wide.0 = 40;
    println!("field {field} wide {}", wide.0);

    for i in 5..7 {
        let each = &make(i);
        println!("turn {}", each.0);
    }

    let value = {
        let inner = &Trace(8);
        inner.0 * 2
    };
    println!("value {value}");

    let local = Trace(9);
    let lent = &local;
    println!("lent {}", lent.0);
}
