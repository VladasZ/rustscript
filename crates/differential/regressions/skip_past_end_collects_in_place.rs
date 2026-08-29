// std collects a `vec.into_iter().map(..).skip(n)` into a `Vec` in place by index, so a skip past
// the end never runs the closure and the overflow inside it never happens. A slice `iter()`,
// a `sum` or a `take` drain lazily and do run it. Seed 20685104381.

fn diff_opaque_i64(value: i64) -> i64 {
    value
}

fn main() {
    for item in vec![diff_opaque_i64(1)].into_iter().map(|_| vec![diff_opaque_i64(9223372036854775807), diff_opaque_i64(3289267416785484334)].iter().copied().sum::<i64>()).skip(2usize).collect::<Vec<i64>>() {
        println!("{item}");
    }
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 {x}"); x }).skip(2).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip1 {x}"); x }).skip(1).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip1 skip1 {x}"); x }).skip(1).skip(1).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 map {x}"); x }).skip(2).map(|x| x + 1).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 string {x}"); x.to_string() }).skip(2).collect::<Vec<String>>();
    println!("{a:?}");
    let a = [diff_opaque_i64(1), 2].iter().map(|x| { println!("slice2 skip2 {x}"); *x }).skip(2).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 sum {x}"); x }).skip(2).sum::<i64>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 take {x}"); x }).skip(2).take(1).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().filter(|_| true).map(|x| { println!("filter2 skip2 {x}"); x }).skip(2).collect::<Vec<i64>>();
    println!("{a:?}");
    let a = vec![diff_opaque_i64(1), 2].into_iter().map(|x| { println!("vec2 skip2 enumerate {x}"); x }).skip(2).enumerate().collect::<Vec<(usize, i64)>>();
    println!("{a:?}");
}
