pub fn rotated_indices(length: usize, round: usize) -> Vec<usize> {
    (0..length)
        .map(|offset| (round + offset) % length)
        .collect()
}

pub fn parse_compute_ns(stderr: &str) -> Option<f64> {
    stderr.lines().find_map(|line| {
        let start = line.find("COMPUTE_NS")? + "COMPUTE_NS".len();
        let digits: String = line[start..]
            .chars()
            .skip_while(|character| !character.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

pub fn parse_rss_bytes(stderr: &str) -> Option<u64> {
    for line in stderr.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("maximum resident set size") {
            let digits: String = line.chars().filter(char::is_ascii_digit).collect();
            let value: u64 = digits.parse().ok()?;
            return Some(if lower.contains("kbytes") {
                value * 1_024
            } else {
                value
            });
        }
    }
    None
}
