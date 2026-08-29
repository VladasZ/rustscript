// `rev` is lazy, a `map` closure runs from the back, a `skip` after it never touches what it
// skips, and `next_back` interleaves with `next`. Found beside seed 20685104381.

fn diff_opaque_i64(v: i64) -> i64 {
    v
}
fn main() {
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("A rev {x}"); x }).rev().skip(1).collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("B rev {x}"); x }).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = [diff_opaque_i64(1), 2, 3].iter().map(|x| { println!("C rev {x}"); *x }).rev().take(2).collect::<Vec<i64>>(); println!("{a:?}");
    let a = (diff_opaque_i64(1)..4).map(|x| { println!("D rev {x}"); x }).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = (diff_opaque_i64(1)..=4).map(|x| { println!("E rev {x}"); x }).rev().skip(2).collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3, 4].into_iter().filter(|x| { println!("F rev {x}"); x % 2 == 0 }).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3, 4].into_iter().map(|x| { println!("G rev {x}"); x }).enumerate().rev().collect::<Vec<(usize, i64)>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3, 4].into_iter().map(|x| { println!("H rev {x}"); x }).skip(1).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3, 4].into_iter().map(|x| { println!("I rev {x}"); x }).take(2).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("J rev {x}"); x }).chain(vec![diff_opaque_i64(3), 4].into_iter().map(|x| { println!("J2 rev {x}"); x })).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("K rev {x}"); x }).zip(vec![diff_opaque_i64(7), 8].into_iter().map(|x| { println!("K2 rev {x}"); x })).rev().collect::<Vec<(i64, i64)>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("L rev {x}"); x }).rev().rev().collect::<Vec<i64>>(); println!("{a:?}");
    let mut it = vec![diff_opaque_i64(1), 2, 3, 4].into_iter().map(|x| { println!("M rev {x}"); x });
    println!("{:?} {:?} {:?} {:?} {:?}", it.next(), it.next_back(), it.next(), it.next_back(), it.next());
    let a = "héllo".chars().rev().collect::<String>(); println!("{a}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("N rev {x}"); x }).rev().last(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("O rev {x}"); x }).rev().next(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("P rev {x}"); x }).rev().sum::<i64>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("Q rev {x}"); x }).filter_map(|x| if x > 1 { Some(x) } else { None }).rev().collect::<Vec<i64>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("R rev {x}"); x }).rev().enumerate().collect::<Vec<(usize, i64)>>(); println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2, 3].into_iter().map(|x| { println!("S rev {x}"); x }).rev().skip(3).collect::<Vec<i64>>(); println!("{a:?}");
}
