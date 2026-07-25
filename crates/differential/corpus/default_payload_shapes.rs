// `unwrap_or_default` has no turbofish, so its payload type comes from
// wherever the source states it. These shapes are the ones the generator
// writes and clippy rejects as non-idiomatic, so they live here rather than
// in an example.

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn main() {
    let flag = diff_opaque_u64(0) > 1000;

    // The argument that built the Option names the payload.
    println!("{:?}", flag.then_some(126i8).unwrap_or_default());
    println!("{:?}", flag.then_some(1.5f32).unwrap_or_default());
    println!("{:?}", flag.then_some(true).unwrap_or_default());
    println!("{:?}", flag.then_some('x').unwrap_or_default());
    println!("{:?}", flag.then_some(None::<f64>).unwrap_or_default());
    println!("{:?}", flag.then_some(vec![1u8]).unwrap_or_default());
    println!("{:?}", flag.then_some(Some(7u16)).unwrap_or_default());
    println!("{:?}", (flag.then_some(126i8).unwrap_or_default() as u16));

    // `or` keeps the payload both sides share.
    println!(
        "{:?}",
        flag.then_some(vec![true])
            .or(None::<Vec<bool>>)
            .unwrap_or_default()
    );

    // An unwrap whose result is unwrapped again must have produced an Option,
    // so the inner default is None whatever it wraps.
    let chained: u16 = flag
        .then_some(Some(0u16))
        .unwrap_or_default()
        .unwrap_or_default();
    println!("{chained:?}");

    // A `None` that states its own payload.
    println!("{:?}", None::<u64>.unwrap_or_default());
    println!("{:?}", None::<Option<f64>>.unwrap_or_default());
    println!("{:?}", None::<Vec<u8>>.unwrap_or_default());
    println!("{:?}", None::<u32>.as_ref().cloned().unwrap_or_default());

    // Nested Options: each unwrap peels one layer, so the payload has to
    // survive more than one level to type the last one.
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
