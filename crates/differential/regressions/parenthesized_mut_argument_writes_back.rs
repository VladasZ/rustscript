// written by hand, a `&mut` argument in parentheses writes back like a bare one

fn diff_inc(n: &mut i32) {
    *n += 1;
}

fn diff_push(v: &mut Vec<i64>, n: i64) {
    v.push(n);
}

fn main() {
    let mut k = 1i32;
    diff_inc((&mut k));
    diff_inc(&mut k);
    println!("k {k}");
    let mut v: Vec<i64> = Vec::new();
    diff_push((&mut v), 2);
    diff_push(&mut v, 3);
    println!("v {:?}", v);
}
