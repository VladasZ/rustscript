use std::env;
use std::process::Command;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    let helper = &args[1];
    let launches: u64 = args[2].parse().unwrap();
    let t = Instant::now();
    let mut checksum: u64 = 0;
    for i in 0..launches {
        let output = Command::new(helper).arg(i.to_string()).output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let value: u64 = text.trim().parse().unwrap();
        checksum = (checksum + value) % 1_000_000_007;
    }
    let ns = t.elapsed().as_nanos();
    println!("launches={launches} checksum={checksum}");
    eprintln!("COMPUTE_NS {ns}");
}
