#!/usr/bin/env rustscript

fn quicksort(list: Vec<i64>) -> Vec<i64> {
    if list.len() <= 1 {
        return list;
    }
    let pivot = list[0];
    let rest: Vec<i64> = list.iter().skip(1).cloned().collect();
    let less: Vec<i64> = rest.iter().filter(|x| **x < pivot).cloned().collect();
    let more: Vec<i64> = rest.iter().filter(|x| **x >= pivot).cloned().collect();
    let mut out = quicksort(less);
    out.push(pivot);
    out.extend(quicksort(more));
    out
}

fn main() {
    let data = vec![5, 2, 9, 1, 7, 3, 8, 4, 6];
    println!("{:?}", quicksort(data));
}
