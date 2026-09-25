// A binding a closure writes lives in a capture cell. Reading a `Copy` value out of it copies,
// so a closure that reads the cell later still finds the value.
// Found by seeds 20716106820 and 20717103347.

fn main() {
    let mut v: i32 = 3;
    let mut bump = |_p: usize| -> i32 {
        v += 1;
        v
    };
    println!("{}", bump(0));
    v = vec![1.0f64].into_iter().map(|_x: f64| v).fold(v, |_acc, x| x);
    println!("{v}");
    v = vec![1.0f64].into_iter().map(|_x: f64| v).fold(v, |_acc, x| x).reverse_bits();
    println!("{v}");
}
