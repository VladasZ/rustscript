#!/usr/bin/env rust

//! `Drop` follows ownership. A value moved into an inner scope drops there, a consumed
//! iterator drops what it still holds when it is dropped, and a handed out item drops with its
//! new owner.

struct Tag(&'static str);

impl Drop for Tag {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn make() -> Tag {
    let _tmp = Tag("temp");
    Tag("made")
}

fn main() {
    let _outer = Tag("outer");
    let made = make();
    {
        let _inner = Tag("inner");
        let moved = made;
        println!("holding {}", moved.0);
    }
    println!("after inner scope");

    let tags = vec![Tag("v1"), Tag("v2"), Tag("v3")];
    let mut rest = tags.into_iter();
    let first = rest.next();
    drop(rest);
    println!("iterator dropped");
    drop(first);
    println!("first dropped");

    let more = vec![Tag("w1"), Tag("w2")];
    for tag in more {
        println!("loop sees {}", tag.0);
        if tag.0 == "w1" {
            break;
        }
    }
    println!("after loop");

    let kept = vec![Tag("k1"), Tag("k2")];
    let moved = kept;
    println!("moved holds {}", moved.len());
    println!("end of main");
}
