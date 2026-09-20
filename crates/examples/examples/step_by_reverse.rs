fn main() {
    for len in 0..9 {
        for step in 1..5 {
            let values: Vec<i32> = (0..len)
                .map(|n| {
                    println!("make {n}");
                    n * 2
                })
                .step_by(step)
                .rev()
                .collect();
            println!("{len} {step}: {values:?}");
            let mut mixed = (0..len)
                .map(|n| {
                    println!("mixed {n}");
                    n * 2
                })
                .step_by(step);
            println!("{:?}", mixed.next());
            println!("{:?}", mixed.next_back());
            println!("{:?}", mixed.next());
            println!("{:?}", mixed.rev().collect::<Vec<_>>());
        }
    }
}
