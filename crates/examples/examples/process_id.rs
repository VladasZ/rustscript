// std::process::id() returns the current process id. The value differs per run, so this prints only a
// stable property, that it is a positive number, so compiled and interpreted output matches.

fn main() {
    let pid = std::process::id();
    println!("positive: {}", pid > 0);
}
