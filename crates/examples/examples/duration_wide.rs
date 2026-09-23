use std::hint::black_box;
use std::time::Duration;

fn main() {
    let d = Duration::new(u64::MAX, 0);
    println!("{}", d.as_secs());
    println!("{}", d.as_millis());
    println!("{}", d.as_nanos());
    println!("{:?}", d.checked_add(Duration::from_secs(1)));
    let big = Duration::from_secs(u64::MAX - 3) + Duration::from_millis(2_500);
    println!("{} {}", big.as_secs(), big.subsec_millis());
    let half = Duration::from_secs_f64(1.5);
    println!("{half:?} {}", half.as_millis());
    let tiny = Duration::from_secs_f32(0.25);
    println!("{tiny:?} {}", tiny.as_micros());
    let x: u8 = black_box(250);
    println!("{}", x.checked_add(black_box(10)).is_none());
    let v = black_box(vec![1, 2, 3]);
    println!("{}", v.len());
}
