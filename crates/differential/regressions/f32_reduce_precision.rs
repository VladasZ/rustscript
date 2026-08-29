// A `sum` or `product` over `f32` must round after every step like the native `f32` does, so the
// last digits match and `f32::MAX + f32::MAX` overflows to `inf` before `NEG_INFINITY` joins.
// Seeds 20687017526, 20688007867.

fn diff_opaque_f32(value: f32) -> f32 {
    value
}

fn main() {
    let v: Vec<f32> = vec![
        diff_opaque_f32(0.1),
        diff_opaque_f32(0.2),
        diff_opaque_f32(0.3),
        diff_opaque_f32(1234.567),
        diff_opaque_f32(-6969.31),
        diff_opaque_f32(0.7),
    ];
    println!("{}", v.iter().sum::<f32>());
    println!("{}", v.iter().copied().sum::<f32>());
    println!("{}", v.iter().product::<f32>());
    println!("{}", v.iter().copied().product::<f32>());
    println!("{}", v.iter().fold(0.0f32, |a, b| a + b));
    println!("{}", v.iter().fold(1.0f32, |a, b| a * b));
    println!("{}", vec![1.0e20f32, 1.0e20, 1.0e-20].iter().product::<f32>());
    println!("{}", [f32::MAX, f32::MAX, f32::NEG_INFINITY].iter().sum::<f32>());
    println!("{}", [f32::MAX, f32::MAX, f32::NEG_INFINITY].iter().copied().sum::<f32>());
    println!("{}", [f32::MAX, 2.0, 0.0].iter().product::<f32>());
    println!("{:?}", Vec::<f32>::new().iter().sum::<f32>());
    println!("{:?}", Vec::<f32>::new().iter().product::<f32>());
    println!("{:?}", vec![-0.0f32].iter().sum::<f32>());
    println!("{:?}", vec![0.1f64, 0.2].iter().sum::<f64>());
    println!("{:?}", (1..=10).map(|i| i as f32 * 0.1).sum::<f32>());
}
