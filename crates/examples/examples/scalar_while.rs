//! Loops the scalar while plan specializes: int-only `while` and `loop`
//! regions closed by a backward jump, condition included. Each block also
//! exercises one edge the plan must keep identical to the generic path:
//! width-tagged registers, integer methods in the body, break and continue,
//! nested while loops, a mid-loop fallback on a float read, and shapes that
//! stay generic, a call in the body and a `while let` head.

fn steps(start: u64) -> u64 {
    let mut n = start;
    let mut count: u64 = 0;
    while n != 1 {
        if n.is_multiple_of(2) {
            n /= 2;
        } else {
            n = 3 * n + 1;
        }
        count += 1;
    }
    count
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn main() {
    let mut total: u64 = 0;
    for start in 1..2000u64 {
        total += steps(start);
    }
    println!("collatz steps {total}");

    println!("gcd {}", gcd(1_071_000_000, 462_000_123));

    // Nested while loops in one region, both scalar.
    let mut pairs: u64 = 0;
    let mut i: u64 = 0;
    while i < 40 {
        let mut j = i;
        while j < 40 {
            if (i * j).is_multiple_of(7) {
                pairs += 1;
            }
            j += 1;
        }
        i += 1;
    }
    println!("pairs {pairs}");

    // A continue jumps back to the head from inside the body, its own
    // backward jump must not become a plan for half the loop.
    let mut n: i64 = 0;
    let mut skipped: i64 = 0;
    let mut seen: i64 = 0;
    while n < 500 {
        n += 1;
        if n % 3 == 0 {
            skipped += 1;
            continue;
        }
        seen += 1;
    }
    println!("skipped {skipped} seen {seen}");

    // A `loop` with a break is the same backward-jump region.
    let mut doubling: u32 = 1;
    let mut rounds: u32 = 0;
    loop {
        if doubling > 10_000 {
            break;
        }
        doubling = doubling.saturating_mul(2);
        rounds += 1;
    }
    println!("doubling {doubling} rounds {rounds}");

    // Integer methods in the body, width-tagged and plain.
    let mut clamped_sum: u64 = 0;
    let mut edges: u64 = 0;
    let mut bits: u32 = 0;
    let mut k: u64 = 0;
    while k < 1000 {
        clamped_sum += (k * 7 % 100).clamp(10, 80);
        edges += (k % 50).max(12) + (k % 60).min(40);
        bits += k.count_ones();
        k += 1;
    }
    println!("clamped {clamped_sum} edges {edges} bits {bits}");

    // The plan runs until the branch compares two float registers it
    // cannot read, then the generic path must resume with every register
    // exactly where the plan left them. A float literal in the loop would
    // reject the plan outright, registers loaded outside it do not.
    let scale = 2.5f64;
    let half = 2.0f64;
    let mut caught: i64 = 0;
    let mut ticks: i64 = 0;
    while ticks < 300 {
        ticks += 1;
        if ticks == 290 && scale > half {
            caught = 1;
        }
    }
    println!("ticks {ticks} caught {caught}");

    // A call in the body keeps the loop on the generic path.
    let mut called: u64 = 0;
    let mut m: u64 = 1;
    while m < 20 {
        called += steps(m);
        m += 1;
    }
    println!("called {called}");

    // A `while let` head tests and binds, which stays generic.
    let mut stack = vec![3, 9, 27, 81];
    let mut drained: i64 = 0;
    while let Some(top) = stack.pop() {
        drained += top;
    }
    println!("drained {drained}");

    // An unsigned width crossing the i64 midpoint keeps its real value.
    let mut big: u64 = u64::MAX - 5000;
    let mut wraps: u64 = 0;
    while big != u64::MAX {
        big = big.wrapping_add(1);
        wraps += 1;
    }
    println!("wraps {wraps} big {big}");
}
