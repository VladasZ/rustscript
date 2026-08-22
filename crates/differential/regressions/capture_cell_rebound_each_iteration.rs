//! A binding inside a loop starts a new capture cell every iteration. The
//! interpreter once kept the first cell, so later bindings read the value
//! the closure left behind. Every binding form that can rerun is here.

fn opaque(value: i64) -> i64 {
    value
}

fn main() {
    for step in 0..3i64 {
        let mut total: i64 = opaque(step * 10);
        let mut add = || total += 1;
        add();
        println!("let {total}");
    }

    for mut item in [10i64, 20, 30] {
        let mut bump = || item += 1;
        bump();
        println!("for {item}");
    }

    let mut queue = vec![1i64, 2, 3];
    while let Some(mut taken) = queue.pop() {
        let mut raise = || taken += 100;
        raise();
        println!("while let {taken}");
    }

    let mut left = 3i64;
    while left > 0 {
        let mut seen: Vec<i64> = Vec::new();
        let mut push = || seen.push(left);
        push();
        println!("while {seen:?}");
        left -= 1;
    }

    for step in 0..3i64 {
        match Some(opaque(step * 10)) {
            Some(mut found) => {
                let mut grow = || found += 1;
                grow();
                println!("match {found}");
            }
            None => println!("match none"),
        }
    }

    // A `DropCell` shifts every jump target past it, so these loops jump in
    // every direction over the bindings that carry one.
    for step in 0..4i64 {
        if step % 2 == 0 {
            continue;
        }
        let mut odd: i64 = opaque(step * 100);
        let mut raise = || odd += 1;
        raise();
        println!("continue {odd}");
    }

    for outer in 0..2i64 {
        for inner in 0..2i64 {
            let mut both: i64 = opaque(outer * 10 + inner);
            let mut raise = || both += 1;
            raise();
            println!("nested {both}");
        }
    }

    for step in 0..3i64 {
        let mut held: i64 = opaque(step);
        loop {
            let mut doubled: i64 = held * 2;
            let mut raise = || doubled += 1;
            raise();
            held = doubled;
            break;
        }
        let mut lift = || held += 100;
        lift();
        println!("break {held}");
    }
}
