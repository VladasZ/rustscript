// Scalar chain reductions and for plan vec pushes, early exits and fallback shapes included.

fn main() {
    // the specialized shapes
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

    // early exits consume up to and including the match, the rest stays
    let probe: Vec<i64> = vec![1, 5, 3, 9, 2];
    let mut it = probe.iter();
    let found = it.by_ref().any(|a| *a > 4);
    let rest: Vec<i64> = it.copied().collect();
    println!("found={found} rest={rest:?}");
    let mut it2 = probe.iter();
    let all = it2.by_ref().all(|a| *a < 4);
    let rest2: Vec<i64> = it2.copied().collect();
    println!("all={all} rest2={rest2:?}");

    // a mutable capture and non integer elements fall back
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

    let e: Vec<i64> = Vec::new();
    let es: i64 = e.iter().sum();
    let ec = e.iter().filter(|a| **a > 0).count();
    let ea = e.iter().any(|a| *a > 0);
    let el = e.iter().all(|a| *a > 0);
    println!("es={es} ec={ec} ea={ea} el={el}");

    // a typed sum keeps its width
    let small: Vec<u8> = vec![10, 20, 30];
    let ts: u8 = small.iter().map(|a| *a).sum::<u8>();
    println!("ts={ts}");

    // push loops
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

    // a nested push loop rolls its inner pushes into the outer iteration
    let mut pairs: Vec<i64> = Vec::new();
    for i in 0..4 {
        for j in 0..3 {
            pairs.push(i * 10 + j);
        }
    }
    println!("pairs={pairs:?}");

    // a non scalar push falls back
    let mut names: Vec<String> = Vec::new();
    for i in 0..3 {
        names.push(format!("n{i}"));
    }
    println!("names={names:?}");
}
