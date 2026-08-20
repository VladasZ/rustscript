// Parse errors are real error values with the std `Debug` and `Display`
// forms, a bare `sum` or `product` runs in the element width and keeps a
// `u64` past i64::MAX, and an annotated `let` sum overflows at its width.
fn main() {
    let e = "x".parse::<i32>().unwrap_err();
    let f = "".parse::<f64>().unwrap_err();
    let g = "999".parse::<u8>().unwrap_err();
    println!("{e:?} {e} {f:?} {f} {g:?} {g} {}", e == "y".parse::<i32>().unwrap_err());
    println!("{e:#?}");
    println!("{:?} {:?}", "12".parse::<i32>(), "zz".parse::<bool>());
    let v: Vec<u64> = vec![18446744073709551614];
    println!("{} {}", v.iter().product::<u64>(), v.iter().copied().sum::<u64>());
    let w: Vec<u32> = vec![4294967295, 1];
    let s: u64 = w.iter().map(|x| u64::from(*x)).sum();
    println!("{s}");
    let bytes: Vec<u8> = vec![200, 100];
    let total: u8 = bytes.iter().sum();
    println!("{total}");
}
