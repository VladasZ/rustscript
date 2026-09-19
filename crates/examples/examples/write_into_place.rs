#!/usr/bin/env rust

// `write!` into a captured string, a field or an element stores the grown buffer back into
// its place, so nothing written is lost.

use std::fmt::Write;

struct Report {
    body: String,
    lines: u32,
}

impl Report {
    fn add(&mut self, text: &str) {
        writeln!(self.body, "{}: {text}", self.lines).unwrap();
        self.lines += 1;
    }
}

fn main() {
    let mut buf = String::new();
    let mut wr = |i: i32| write!(buf, "{i},").unwrap();
    wr(1);
    wr(2);
    write!(buf, "3,").unwrap();
    println!("{buf}");

    let mut report = Report {
        body: String::new(),
        lines: 0,
    };
    report.add("first");
    report.add("second");
    write!(report.body, "end").unwrap();
    print!("{}", report.body);
    println!(" ({} lines)", report.lines);

    let mut parts: Vec<String> = (0..2).map(|_| String::new()).collect();
    write!(parts[0], "a").unwrap();
    write!(parts[1], "b").unwrap();
    writeln!(parts[0], "c").unwrap();
    print!("{}{}", parts[0], parts[1]);
    println!();

    let mut log = String::new();
    let mut note = |tag: &str| {
        writeln!(log, "[{tag}]").unwrap();
    };
    note("x");
    note("y");
    print!("{log}");
}
