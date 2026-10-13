// A method that keeps its argument takes it out of the argument window. The caller's value
// behind a `&x` argument or a forwarded reference parameter must stay where it is, it read as
// `()` after `held.push(&text)`. Same family as seed 20736118481.

use std::collections::HashMap;

struct DiffTrace(i64);

impl Drop for DiffTrace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn two(x: &i64) -> usize {
    let mut m: HashMap<&i64, &i64> = HashMap::new();
    m.insert(x, x);
    m.len() + (*x as usize)
}

fn one(x: &i64) -> usize {
    let mut m: HashMap<&i64, i64> = HashMap::new();
    m.insert(x, 1);
    m.len() + (*x as usize)
}

fn pushes(x: &i64) -> usize {
    let mut s: Vec<&i64> = Vec::new();
    s.push(x);
    s.push(x);
    s.len() + (*x as usize)
}

fn twice(&x: &i64) -> usize {
    let mut m: HashMap<i64, i64> = HashMap::new();
    m.insert(x, x);
    m.insert(x, 1);
    m.len()
}

fn main() {
    let trace = DiffTrace(1);
    println!("{}", trace.0);
    let text = String::from("abc");
    let mut held: Vec<&String> = Vec::new();
    held.push(&text);
    println!("push {} {:?}", text, held);
    let mut by_key: HashMap<u8, &String> = HashMap::new();
    by_key.insert(1, &text);
    println!("insert {} {:?}", text, by_key);
    let n = 5i64;
    println!("one {} two {} pushes {} twice {}", one(&n), two(&n), pushes(&n), twice(&n));
    println!("n {n}");
    let nums = vec![4i64, 5];
    let a: Vec<usize> = nums
        .iter()
        .map(|x: &i64| {
            let mut m: HashMap<&i64, &i64> = HashMap::new();
            m.insert(x, x);
            m.insert(x, x);
            m.len() + (*x as usize)
        })
        .collect();
    println!("a {:?} {:?}", a, nums);
    let d: Vec<i64> = vec![1i64]
        .into_iter()
        .filter(|&x: &i64| {
            let mut m: HashMap<i64, i64> = HashMap::new();
            m.insert(x, x);
            m.insert(x, 1);
            m.len() == 1
        })
        .collect();
    println!("d {:?}", d);
}
