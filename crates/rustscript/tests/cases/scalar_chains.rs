// Pins the scalar chain reductions and the for-plan vec pushes against the
// compiler: adaptor chains keep generic consumption semantics, early exits
// leave the source where real iterators leave it, and every fallback shape
// answers the same values as the specialized path.

fn main() {
    // The specialized shapes: map and filter stages into sum, count, any,
    // and all, over a vec and over a range.
    let mut x: i64 = 12345;
    let mut v: Vec<i64> = Vec::new();
    for _ in 0..1000 {
        x = x * 48271 % 2_147_483_647;
        v.push(x % 1000);
    }
    let sum: i64 = v.iter().map(|a| a * 3 + 1).filter(|a| a % 2 == 0).sum();
    let count = v.iter().filter(|a| **a > 500).count();
    let any_big = v.iter().any(|a| *a > 995);
    let all_small = v.iter().all(|a| *a < 990);
    println!("sum={sum} count={count} any={any_big} all={all_small}");
    let rs: i64 = (1..=100).filter(|n| n % 3 == 0).map(|n| n * n).sum();
    println!("rs={rs}");

    // Early exits consume up to and including the match, the rest stays.
    let probe: Vec<i64> = vec![1, 5, 3, 9, 2];
    let mut it = probe.iter();
    let found = it.by_ref().any(|a| *a > 4);
    let rest: Vec<i64> = it.copied().collect();
    println!("found={found} rest={rest:?}");
    let mut it2 = probe.iter();
    let all = it2.by_ref().all(|a| *a < 4);
    let rest2: Vec<i64> = it2.copied().collect();
    println!("all={all} rest2={rest2:?}");

    // A captured immutable int folds into the plan, a mutable capture and
    // non-integer elements fall back to the generic path.
    let k = 10i64;
    let ks: i64 = probe.iter().map(|a| a * k).sum();
    let mut seen = 0i64;
    let odd = probe
        .iter()
        .filter(|a| {
            seen += 1;
            **a % 2 == 1
        })
        .count();
    let words = vec!["a".to_string(), "bb".to_string(), "ccc".to_string()];
    let letters: i64 = words.iter().map(|w| w.len() as i64).sum();
    println!("ks={ks} odd={odd} seen={seen} letters={letters}");

    // Empty sources.
    let e: Vec<i64> = Vec::new();
    let es: i64 = e.iter().sum();
    let ec = e.iter().filter(|a| **a > 0).count();
    let ea = e.iter().any(|a| *a > 0);
    let el = e.iter().all(|a| *a > 0);
    println!("es={es} ec={ec} ea={ea} el={el}");

    // A typed sum keeps its width.
    let small: Vec<u8> = vec![10, 20, 30];
    let ts: u8 = small.iter().map(|a| *a).sum::<u8>();
    println!("ts={ts}");

    // Push loops: two vecs in one loop, a break, a continue, and floats.
    let mut evens: Vec<i64> = Vec::new();
    let mut halves: Vec<f64> = Vec::new();
    for i in 0..20 {
        if i == 15 {
            break;
        }
        if i % 2 != 0 {
            continue;
        }
        evens.push(i * i);
        halves.push(i as f64 / 2.0);
    }
    println!("evens={evens:?} halves={halves:?}");

    // A nested push loop rolls all its inner pushes into the outer
    // iteration.
    let mut pairs: Vec<i64> = Vec::new();
    for i in 0..4 {
        for j in 0..3 {
            pairs.push(i * 10 + j);
        }
    }
    println!("pairs={pairs:?}");

    // A push of a non-scalar value falls back to the generic loop.
    let mut names: Vec<String> = Vec::new();
    for i in 0..3 {
        names.push(format!("n{i}"));
    }
    println!("names={names:?}");
}
