// 2 `SystemTime` values were equal only when they were the same handle, so a watch loop that
// compared `(usize, SystemTime)` saw a change on every round. A time had no order, no `max`, and
// could not be moved by a `Duration`. Found by `build/shared/src/hot.rs` of hilen.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn newest(times: &[SystemTime], state: &mut (usize, SystemTime)) {
    for time in times {
        state.0 += 1;
        state.1 = state.1.max(*time);
    }
}

fn secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn main() {
    let older = UNIX_EPOCH + Duration::from_secs(5);
    let newer = UNIX_EPOCH + Duration::from_secs(7);
    println!("{} {}", secs(older.max(newer)), secs(newer.max(older)));
    println!("{} {}", secs(older.min(newer)), secs(newer.min(older)));
    println!("{} {} {}", older == newer, older != newer, older == older);
    println!("{} {} {} {}", older < newer, older <= newer, older > newer, older >= older);
    println!("{}", older == UNIX_EPOCH + Duration::from_secs(5));
    println!("{}", newer - Duration::from_secs(2) == older);
    println!("{:?}", older.cmp(&newer));

    let mut first = (0, UNIX_EPOCH);
    newest(&[older, newer, older], &mut first);
    let mut second = (0, UNIX_EPOCH);
    newest(&[newer, older, older], &mut second);
    println!("{} {}", first.0, secs(first.1));
    println!("{} {}", first == second, first != second);
    let mut third = (0, UNIX_EPOCH);
    newest(&[older, older, older], &mut third);
    println!("{} {}", first == third, first != third);
    println!("{}", (1usize, older) == (2usize, older));

    let start = Instant::now();
    let later = start + Duration::from_millis(250);
    println!("{} {} {}", later > start, later == start, start == start);
    println!("{:?} {:?}", later - start, start - later);
    println!("{}", (later - Duration::from_millis(250)) == start);
    println!("{}", later.max(start) == later);
}
