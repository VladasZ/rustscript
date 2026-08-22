// The id differs per run, so only a stable property is printed.

fn main() {
    let pid = std::process::id();
    println!("positive: {}", pid > 0);
}
