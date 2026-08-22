// `unwrap_or_default` shapes the generator writes and clippy rejects, so they live here and not
// in an example.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    let flag = diff_opaque_u64(0) > 1000;

    // the argument that built the Option
    println!("{:?}", flag.then_some(126i8).unwrap_or_default());
    println!("{:?}", flag.then_some(1.5f32).unwrap_or_default());
    println!("{:?}", flag.then_some(true).unwrap_or_default());
    println!("{:?}", flag.then_some('x').unwrap_or_default());
    println!("{:?}", flag.then_some(None::<f64>).unwrap_or_default());
    println!("{:?}", flag.then_some(vec![1u8]).unwrap_or_default());
    println!("{:?}", flag.then_some(Some(7u16)).unwrap_or_default());
    println!("{:?}", (flag.then_some(126i8).unwrap_or_default() as u16));

    // `or`
    println!(
        "{:?}",
        flag.then_some(vec![true])
            .or(None::<Vec<bool>>)
            .unwrap_or_default()
    );

    // an unwrap unwrapped again must have produced an Option
    let chained: u16 = flag
        .then_some(Some(0u16))
        .unwrap_or_default()
        .unwrap_or_default();
    println!("{chained:?}");

    // `None::<T>`
    println!("{:?}", None::<u64>.unwrap_or_default());
    println!("{:?}", None::<Option<f64>>.unwrap_or_default());
    println!("{:?}", None::<Vec<u8>>.unwrap_or_default());
    println!("{:?}", None::<u32>.as_ref().cloned().unwrap_or_default());

    // nested Options
    println!("{:?}", Some(None::<f64>).unwrap_or_default().unwrap_or_default());
    println!(
        "{:?}",
        Some(Some(None::<u8>))
            .unwrap_or_default()
            .unwrap_or_default()
            .unwrap_or_default()
    );
    println!(
        "{:?}",
        Some(None::<Vec<u8>>).unwrap_or_default().unwrap_or_default()
    );
}
