#!/usr/bin/env rust


use std::collections::{BTreeMap, HashMap};
#[derive(Debug)]
struct T(i32);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn check(x: i32) -> Result<i32, String> {
    println!("check {x}");
    if x < 0 {
        Err(format!("neg {x}"))
    } else {
        Ok(x * 2)
    }
}
#[allow(clippy::map_collect_result_unit)]
fn all_ok(v: &[i32]) -> Result<(), String> {
    v.iter().map(|x| check(*x).map(|_| ())).collect()
}
fn parse_pairs(s: &str) -> Result<BTreeMap<String, i32>, String> {
    s.split(',')
        .map(|kv| {
            let (k, v) = kv.split_once('=').ok_or("no =")?;
            Ok((k.to_string(), v.parse::<i32>().map_err(|e| e.to_string())?))
        })
        .collect()
}
fn run() -> Result<Vec<i32>, String> {
    let v: Vec<i32> = [1, 2, 3]
        .iter()
        .map(|x| check(*x))
        .collect::<Result<_, _>>()?;
    println!("{v:?}");
    let w: Vec<i32> = [4, -1, 5]
        .iter()
        .map(|x| check(*x))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(w)
}
fn main() {
    println!("{:?}", run());
    println!("{:?} {:?}", all_ok(&[1, 2]), all_ok(&[1, -2, 3]));
    println!("{:?} {:?}", parse_pairs("b=2,a=1"), parse_pairs("b=2,a=x"));
    let s: Result<String, char> = "abc"
        .chars()
        .map(|c| {
            if c == 'z' {
                Err(c)
            } else {
                Ok(c.to_ascii_uppercase())
            }
        })
        .collect();
    println!("{s:?}");
    let o: Option<HashMap<i32, i32>> = (1..4).map(|i| Some((i, i * i))).collect();
    println!("{:?}", o.map(|m| m.len()));
    let owned = vec![Ok(T(1)), Ok(T(2)), Err("stop"), Ok(T(3))];
    let r: Result<Vec<T>, &str> = owned.into_iter().collect();
    println!("{r:?}");
    let firsts: Option<Vec<char>> = ["ab", "", "cd"].iter().map(|s| s.chars().next()).collect();
    println!("{firsts:?}");
    let sum_ok: i32 = ["1", "2"]
        .iter()
        .map(|s| s.parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .iter()
        .sum();
    println!("{sum_ok}");
}
