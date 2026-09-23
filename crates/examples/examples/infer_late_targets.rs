fn year_month(started: &str) -> (i32, u32) {
    let parts: Vec<&str> = started.split('-').collect();
    let y = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let m = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (y, m)
}

fn doubled(s: &str) -> Option<i64> {
    let n: i64 = s.parse().ok()?;
    Some(n * 2)
}

fn port(arg: &str) -> u8 {
    let port: u8 = if let Ok(port) = arg.parse() {
        port
    } else {
        println!("fallback for {arg}");
        7
    };
    port
}

fn install(pm: &str, tool: &str) -> Option<Vec<String>> {
    let s = |parts: &[&str]| Some(parts.iter().map(ToString::to_string).collect());
    match pm {
        "scoop" => s(&["scoop", "install", tool]),
        _ => None,
    }
}

fn fallback() -> (i32, u32) {
    (1999, 12)
}

fn chosen(month: Option<&str>) -> (i32, u32) {
    match month {
        Some(m) => {
            let parts: Vec<&str> = m.split('-').collect();
            (
                parts.first().and_then(|s| s.parse().ok()).unwrap_or(2000),
                parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1),
            )
        }
        None => fallback(),
    }
}

fn main() {
    println!(
        "{:?} {:?}",
        year_month("2026-300"),
        year_month("-5000000000")
    );
    println!("{:?} {:?}", doubled("21"), doubled("x"));
    println!("{} {} {}", port("80"), port("300"), port("-1"));
    println!("{:?} {:?}", install("scoop", "gh"), install("apt", "gh"));
    println!(
        "{:?} {:?} {:?}",
        chosen(Some("2024-07")),
        chosen(Some("x-99999999999")),
        chosen(None)
    );
}
