use std::time::Instant;

enum Tree {
    Leaf,
    Node(Box<Tree>, Box<Tree>),
}

fn make(depth: i64) -> Tree {
    if depth == 0 {
        Tree::Leaf
    } else {
        Tree::Node(Box::new(make(depth - 1)), Box::new(make(depth - 1)))
    }
}

fn check(t: &Tree) -> i64 {
    match t {
        Tree::Leaf => 1,
        Tree::Node(l, r) => 1 + check(l) + check(r),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let max: i64 = if args.len() > 1 { args[1].parse().unwrap() } else { 11 };
    let t = Instant::now();
    let mut total: i64 = 0;
    let mut d = 4;
    while d <= max {
        let iters = 1 << (max - d + 2);
        for _ in 0..iters {
            let tree = make(d);
            total += check(&tree);
        }
        d += 2;
    }
    let ns = t.elapsed().as_nanos();
    println!("total {total} depth {max}");
    eprintln!("COMPUTE_NS {ns}");
}
