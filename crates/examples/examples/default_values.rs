#!/usr/bin/env rust

// Every spelling of `Default::default()`: builtin types, qualified paths,
// an annotated let, derived structs and enums, a hand written impl, and a
// struct update that keeps declaration order.

#[derive(Default, Debug, Clone, PartialEq)]
struct Inner {
    x: u8,
    tag: String,
}

#[derive(Default, Debug)]
struct Outer {
    a: Inner,
    b: (u8, char),
    c: Option<Inner>,
    d: Vec<i64>,
}

#[derive(Default, Debug)]
enum Mode {
    #[default]
    Idle,
    Busy(u32),
}

#[derive(Debug)]
struct Custom {
    level: i32,
}

impl Default for Custom {
    fn default() -> Self {
        Custom { level: 7 }
    }
}

fn main() {
    let bytes = Vec::<u8>::default();
    let text = String::default();
    let count = u8::default();
    let ratio = f64::default();
    let pair: (i32, bool) = Default::default();
    let triple = <(i32, bool, char)>::default();
    println!("{bytes:?} {text:?} {count} {ratio} {pair:?} {triple:?}");
    println!("{:?} {:?}", Inner::default(), <Inner>::default());
    let outer = Outer {
        c: Some(Inner {
            x: 3,
            tag: String::from("set"),
        }),
        ..Default::default()
    };
    println!("{outer:?}");
    let base = Outer {
        a: Inner {
            x: 9,
            tag: String::from("base"),
        },
        b: (1, 'z'),
        c: None,
        d: vec![1, 2],
    };
    let updated = Outer {
        b: (2, 'y'),
        ..base
    };
    println!("{updated:?}");
    println!("{:?} {:?}", Mode::default(), Mode::Busy(2));
    println!("{:?} {:?}", Custom::default(), <Custom>::default());
    println!("{}", outer.a == Inner::default());
    println!(
        "{} {:?} {:?} {}",
        outer.b.0,
        outer.c,
        updated.d,
        Custom::default().level
    );
    if let Mode::Busy(n) = Mode::Busy(3) {
        println!("{n}");
    }
}
