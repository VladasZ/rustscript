// `Duration::as_secs_f32` was not in the interpreter, a script that printed how long a step took
// stopped at the check. The f32 form must round like the native f32, so its digits differ from
// the f64 form. Found by `build/ios/hot.rs` of hilen.

use std::time::Duration;

fn main() {
    let elapsed = Duration::from_millis(1500);
    println!("{:.1}", elapsed.as_secs_f32());
    println!("{}", elapsed.as_secs_f32());
    println!("{}", elapsed.as_secs_f64());
    println!("{} {}", elapsed.as_millis(), elapsed.as_micros());
    println!("{} {}", elapsed.subsec_millis(), elapsed.subsec_micros());

    let odd = Duration::new(3, 123_456_789);
    let short: f32 = odd.as_secs_f32();
    let long: f64 = odd.as_secs_f64();
    println!("{short} {long}");
    println!("{short:?} {long:?}");
    println!("{:.3} {:.3}", short, long);
    println!("{}", short * 2.0);
    println!("{}", f64::from(short) == long);
    println!("{} {} {}", odd.as_millis(), odd.as_micros(), odd.as_nanos());
    println!("{} {} {}", odd.subsec_millis(), odd.subsec_micros(), odd.subsec_nanos());

    let big = Duration::new(u64::MAX, 999_999_999);
    println!("{} {}", big.as_secs_f32(), big.as_secs_f64());
    let zero = Duration::ZERO;
    println!("{:?} {:?}", zero.as_secs_f32(), zero.as_secs_f64());
    let total: f32 = [elapsed, odd].iter().map(Duration::as_secs_f32).sum();
    println!("{total}");
}
