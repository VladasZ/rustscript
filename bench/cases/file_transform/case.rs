use std::env;
use std::fs;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    let input = &args[1];
    let output = &args[2];
    let t = Instant::now();
    let text = fs::read_to_string(input).unwrap();
    let mut transformed = String::new();
    let mut lines: u64 = 0;
    for line in text.lines() {
        if line.contains("w000") {
            transformed.push_str(&line.replace("w00", "W"));
            transformed.push('\n');
            lines += 1;
        }
    }
    fs::write(output, transformed).unwrap();
    let saved = fs::read_to_string(output).unwrap();
    let mut checksum: u64 = 0;
    for byte in saved.bytes() {
        checksum = (checksum + byte as u64) % 1_000_000_007;
    }
    let ns = t.elapsed().as_nanos();
    println!("lines={lines} bytes={} checksum={checksum}", saved.len());
    eprintln!("COMPUTE_NS {ns}");
}
