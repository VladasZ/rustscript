use std::time::Instant;

fn main() {
    let w: u64 = 140;
    let h: u64 = 140;
    let max_iter: u64 = 140;
    let t = Instant::now();
    let mut count: u64 = 0;
    for py in 0..h {
        let y0 = (py as f64 / h as f64) * 3.0 - 1.5;
        for px in 0..w {
            let x0 = (px as f64 / w as f64) * 3.0 - 2.0;
            let mut x = 0.0f64;
            let mut y = 0.0f64;
            let mut it: u64 = 0;
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
