use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let size: u32 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        140
    };
    let width: u32 = size;
    let height: u32 = size;
    let max_iter: u32 = 140;
    let t = Instant::now();
    let mut count: u32 = 0;
    for py in 0..height {
        let y0 = (f64::from(py) / f64::from(height)) * 3.0 - 1.5;
        for px in 0..width {
            let x0 = (f64::from(px) / f64::from(width)) * 3.0 - 2.0;
            let mut x = 0.0f64;
            let mut y = 0.0f64;
            let mut it: u32 = 0;
            while x * x + y * y <= 4.0 && it < max_iter {
                let xt = x * x - y * y + x0;
                y = 2.0 * x * y + y0;
                x = xt;
                it += 1;
            }
            if it == max_iter {
                count += 1;
            }
        }
    }
    let ns = t.elapsed().as_nanos();
    println!("in set: {count}");
    eprintln!("COMPUTE_NS {ns}");
}
