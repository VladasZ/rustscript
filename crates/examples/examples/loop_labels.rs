#!/usr/bin/env rust

// Labeled `break` and `continue` leave or restart an outer loop from inside an inner one, a
// labeled block gives `break` a value, and `Drop` still runs for every scope a labeled exit
// leaves.

struct Noisy(u32);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let found = 'search: loop {
        for i in 0..10 {
            if i == 3 {
                break 'search i * 2;
            }
        }
    };
    println!("found {found}");

    let mut pairs = Vec::new();
    'outer: for a in 0..4 {
        for b in 0..4 {
            if b > a {
                continue 'outer;
            }
            if a + b > 4 {
                break 'outer;
            }
            pairs.push((a, b));
        }
    }
    println!("{pairs:?}");

    let mut n = 0;
    let hit = 'scan: loop {
        let mut inner = 0;
        while inner < 5 {
            n += 1;
            inner += 1;
            if n == 7 {
                break 'scan n;
            }
        }
    };
    println!("hit {hit}");

    let mut rounds = 0;
    'again: while rounds < 3 {
        rounds += 1;
        for k in 0..3 {
            if k == 1 {
                continue 'again;
            }
            println!("round {rounds} k {k}");
        }
    }

    let value = 'block: {
        if rounds == 3 {
            break 'block "three";
        }
        "other"
    };
    println!("{value}");

    let mut count = 0;
    'count: loop {
        loop {
            count += 1;
            if count % 2 == 0 {
                continue 'count;
            }
            if count > 5 {
                break 'count;
            }
        }
    }
    println!("count {count}");

    'rows: for i in 0..2 {
        let _row = Noisy(i);
        for j in 0..2 {
            let _cell = Noisy(10 + j);
            if j == 1 {
                continue 'rows;
            }
        }
    }
    let mut tries = 0;
    'early: loop {
        let _a = Noisy(100);
        loop {
            let _b = Noisy(200);
            tries += 1;
            if tries == 3 {
                break 'early;
            }
            if tries % 2 == 1 {
                continue 'early;
            }
        }
    }
    println!("end");
}
