#!/usr/bin/env rust

// Drops on `?` early return, by value arguments, and guards inside
// containers, cells and `Rc`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct Guard {
    name: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        println!("dropping {}", self.name);
    }
}

fn guard(name: &str) -> Guard {
    Guard {
        name: name.to_string(),
    }
}

fn step(fail: bool) -> Result<i64, String> {
    if fail {
        Err("failed".to_string())
    } else {
        Ok(1)
    }
}

fn early(fail: bool) -> Result<i64, String> {
    let _g = guard("early");
    let v = step(fail)?;
    println!("no early return");
    Ok(v)
}

fn consume(g: Guard) {
    println!("consuming {}", g.name);
    println!("consume ends");
}

fn main() {
    println!("ok: {:?}", early(false));
    println!("err: {:?}", early(true));

    let moved = guard("moved");
    consume(moved);
    println!("back in main");

    let v = vec![guard("v0"), guard("v1")];
    println!("vec holds {}", v.len());
    drop(v);

    let mut m = HashMap::new();
    m.insert("k".to_string(), guard("in-map"));
    println!("map holds {}", m.len());
    drop(m);

    let t = (guard("in-tuple"), 3);
    println!("tuple holds {}", t.1);
    drop(t);

    let rc = Rc::new(guard("in-rc"));
    let extra = Rc::clone(&rc);
    drop(rc);
    println!("one handle left");
    drop(extra);

    let cell = RefCell::new(guard("in-cell"));
    println!("cell built");
    drop(cell);

    println!("end of main");
}
