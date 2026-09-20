#[derive(Debug, PartialEq)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn yes() -> bool {
    true
}

fn returning() -> Trace {
    let local = Trace(60);
    match (Trace(61) == Trace(62), 0) {
        (false, _) => {
            let inner = Trace(63);
            if yes() {
                return Trace(64);
            }
            println!("{inner:?}");
        }
        (true, _) => println!("{local:?}"),
    }
    Trace(65)
}

fn main() {
    let mut stack = vec![Trace(1), Trace(2)];
    while let Some(Trace(_)) = stack.pop() {
        if yes() {
            continue;
        }
        println!("after continue");
    }
    println!("after while let");
    for turn in 0..2i64 {
        let local = Trace(10 + turn);
        match (Trace(20 + turn) == Trace(30 + turn), turn) {
            (false, _) => {
                let inner = Trace(40 + turn);
                if yes() {
                    continue;
                }
                println!("{inner:?}");
            }
            (true, _) => println!("{local:?}"),
        }
    }
    let result = 'outer: loop {
        let local = Trace(50);
        match (Trace(51) == Trace(52), 0) {
            (false, _) => {
                let inner = Trace(53);
                if yes() {
                    break 'outer Trace(54);
                }
                println!("{inner:?}");
            }
            (true, _) => println!("{local:?}"),
        }
    };
    println!("{result:?}");
    println!("{:?}", returning());
    let mut first = Trace(70);
    let second = first;
    match (0, 0) {
        (a, b) if yes() => {
            first = Trace(71);
            println!("{a} {b}");
        }
        _ => return,
    }
    println!("{first:?} {second:?}");
}
