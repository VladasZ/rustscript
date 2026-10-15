#!/usr/bin/env rust

//! `split_inclusive` keeps the separator on each piece, `split_terminator` drops one empty piece
//! at the end. A here-string reader walks the lines with their newlines and counts byte offsets.

fn main() {
    let body = "first\nsecond\r\n'@ | pen\ntail";
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        println!("{offset:>2} {line:?}");
        if line.starts_with("'@") {
            println!("rest {:?}", &body[offset + line.len()..]);
        }
        offset += line.len();
    }

    println!("{:?}", "a\nb\n".split_inclusive('\n').collect::<Vec<_>>());
    println!("{:?}", "".split_inclusive('\n').collect::<Vec<_>>());
    println!("{:?}", "a, b, c".split_inclusive(", ").collect::<Vec<_>>());
    println!(
        "{:?}",
        "a-b_c".split_inclusive(['-', '_']).collect::<Vec<_>>()
    );
    println!("{}", "x;y;".split_inclusive(';').count());

    println!("{:?}", "a\nb\n".split_terminator('\n').collect::<Vec<_>>());
    println!("{:?}", "a\n\nb".split_terminator('\n').collect::<Vec<_>>());
    println!("{:?}", "A::B::".split_terminator("::").collect::<Vec<_>>());
    println!("{:?}", "".split_terminator(',').collect::<Vec<_>>());
}
