#!/usr/bin/env rust

// A fallback a combinator does not use drops inside the call, `unwrap_or` on a `Some`, `or` on a
// `Some`, `ok` on an `Err`, and the result of a combinator is a value of its own.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct D(i64);

impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn some(value: D) -> Option<D> {
    if value.0 < 0 { None } else { Some(value) }
}

fn none() -> Option<D> {
    None
}

fn ok(value: D) -> Result<D, D> {
    if value.0 < 0 { Err(value) } else { Ok(value) }
}

fn err(value: D) -> Result<D, D> {
    if value.0 < 0 { Ok(value) } else { Err(value) }
}

fn main() {
    let value_a = some(D(1)).unwrap_or(D(2));
    println!("a {value_a:?}");
    let value_b = none().unwrap_or(D(3));
    println!("b {value_b:?}");
    let value_c = ok(D(4)).unwrap_or(D(5));
    println!("c {value_c:?}");
    let value_d = err(D(6)).unwrap_or(D(7));
    println!("d {value_d:?}");
    let value_e = err(D(8)).unwrap_or_default();
    println!("e {value_e:?}");
    let value_f = ok(D(9)).ok();
    println!("f {value_f:?}");
    let value_g = ok(D(10)).err();
    println!("g {value_g:?}");
    let value_h = some(D(11)).or(some(D(12)));
    println!("h {value_h:?}");
    let value_i = some(D(13)).and(some(D(14)));
    println!("i {value_i:?}");
    let value_j = some(D(15)).xor(some(D(16)));
    println!("j {value_j:?}");
    let value_k = some(D(17)).ok_or(D(18));
    println!("k {value_k:?}");
    let value_l = some(D(19)).map_or(D(20), |x| D(x.0 + 100));
    println!("l {value_l:?}");
    let value_m = ok(D(21)).and(ok(D(22)));
    println!("m {value_m:?}");
    let value_n = ok(D(23)).or(ok(D(24)));
    println!("n {value_n:?}");
    let empty: Vec<D> = Vec::new();
    let value_o = empty.first().cloned().unwrap_or(D(25));
    println!("o {value_o:?}");
    println!("end");
}
