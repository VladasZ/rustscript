//! While loops the scalar plan runs with vec indexing, 1 edge per block.

/// 2 vecs in 1 loop
fn sum_and_double(len: usize) -> (i64, Vec<i64>) {
    let mut source = vec![0i64; len];
    let mut fill: usize = 0;
    let mut seed: i64 = 0;
    while fill < len {
        source[fill] = seed * 7 % 13;
        fill += 1;
        seed += 1;
    }
    let mut doubled = vec![0i64; len];
    let mut sum: i64 = 0;
    let mut idx: usize = 0;
    while idx < len {
        sum += source[idx];
        doubled[idx] = source[idx] * 2;
        idx += 1;
    }
    (sum, doubled)
}

/// the sieve shape
fn count_primes(limit: usize) -> u64 {
    let mut is_prime = vec![true; limit + 1];
    is_prime[0] = false;
    is_prime[1] = false;
    let mut prime = 2;
    while prime * prime <= limit {
        if is_prime[prime] {
            let mut mark = prime * prime;
            while mark <= limit {
                is_prime[mark] = false;
                mark += prime;
            }
        }
        prime += 1;
    }
    let mut count: u64 = 0;
    let mut probe = 2;
    while probe <= limit {
        if is_prime[probe] {
            count += 1;
        }
        probe += 1;
    }
    count
}

/// A clone taken before the loop must not see its writes.
fn split_from_clone(len: usize) -> (Vec<i64>, Vec<i64>) {
    let mut cells = vec![0i64; len];
    let mut fill: usize = 0;
    let mut seed: i64 = 1;
    while fill < len {
        cells[fill] = seed;
        fill += 1;
        seed += 1;
    }
    let before = cells.clone();
    let mut idx: usize = 0;
    while idx < len {
        cells[idx] *= 10;
        idx += 1;
    }
    (cells, before)
}

/// A failover after a write already landed. The journal must undo it, or the generic re-run
/// doubles the increment.
fn journaled_failover(rounds: i64, slots: usize, lo: f64, hi: f64) -> (i64, i64) {
    let mut acc = vec![0i64; slots];
    acc[0] = 100;
    let mut fired: i64 = 0;
    let mut round: i64 = 0;
    while round < rounds {
        let cur = acc[0];
        acc[0] = cur + 1;
        if round == rounds - 10 && hi > lo {
            fired += 1;
        }
        round += 1;
    }
    (acc[0], fired)
}

/// String elements fail over on the first read and leave everything untouched.
fn count_matches(text: &str) -> i64 {
    let mut words: Vec<String> = Vec::new();
    for word in text.split(' ') {
        words.push(word.to_string());
    }
    let total = words.len();
    let mut matches: i64 = 0;
    let mut idx: usize = 0;
    while idx < total {
        if words[idx] == words[0] {
            matches += 1;
        }
        idx += 1;
    }
    matches
}

/// A break right after a write. Only the writes of a failing iteration are undone.
fn toggle_until(len: usize, stop: usize) -> Vec<bool> {
    let mut flags = vec![false; len];
    let mut idx: usize = 0;
    loop {
        if idx >= len {
            break;
        }
        flags[idx] = !flags[idx];
        if idx == stop {
            break;
        }
        idx += 2;
    }
    flags
}

/// u64 elements keep their width
fn sum_widths(len: usize) -> (u64, Vec<u64>) {
    let mut totals = vec![0u64; len];
    let mut idx: usize = 0;
    while idx < len {
        totals[idx] = (idx as u64) * 3 + 1;
        idx += 1;
    }
    let mut grand: u64 = 0;
    let mut probe: usize = 0;
    while probe < len {
        grand += totals[probe];
        probe += 1;
    }
    (grand, totals)
}

/// The journal clears at the boundary a continue jumps to.
fn journal_across_continue(len: usize) -> i64 {
    let mut marks = vec![0i64; len];
    let mut idx: usize = 0;
    while idx < len {
        marks[idx] = 7;
        idx += 1;
        if idx.is_multiple_of(3) {
            continue;
        }
        marks[idx - 1] += 1;
    }
    let mut sum: i64 = 0;
    let mut probe: usize = 0;
    while probe < len {
        sum += marks[probe];
        probe += 1;
    }
    sum
}

fn main() {
    let (sum, doubled) = sum_and_double(8);
    println!("sum {sum} doubled {doubled:?}");
    println!("primes {}", count_primes(3000));
    let (cells, before) = split_from_clone(4);
    println!("cells {cells:?} before {before:?}");
    let (acc, fired) = journaled_failover(40, 2, 1.5, 2.5);
    println!("acc {acc} fired {fired}");
    println!("matches {}", count_matches("aa bb aa"));
    println!("flags {:?}", toggle_until(10, 6));
    let (grand, totals) = sum_widths(5);
    println!("grand {grand} totals {totals:?}");
    println!("msum {}", journal_across_continue(12));
}
