// A by value binding behind a `&` pattern is a copy, in a `for` loop, a `let`, an `if let`, a
// `match` arm and a closure parameter. Dropping or writing the copy leaves the value behind
// the reference whole. Same family as seeds 20732016129 and 20737117830.

struct DiffTrace(i64);

impl Drop for DiffTrace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct Point {
    a: i64,
    b: Option<u8>,
}

fn pick(&x: &Option<i64>) -> usize {
    vec![x; 0usize].len() + Some(x).unwrap_or(x).map_or(0, |v| v as usize)
}

fn main() {
    let trace = DiffTrace(1);
    println!("{}", trace.0);
    let v: Vec<Option<i64>> = vec![Some(1), None, Some(3)];
    let mut out: Vec<usize> = Vec::new();
    for &x in v.iter() {
        out.push(vec![x; 0usize].len());
    }
    for (i, &x) in v.iter().enumerate() {
        out.push(i + vec![x; 0usize].len());
    }
    println!("v1 {:?} {:?}", v, out);
    let o = Some((1u8, 'c'));
    let &y = &o;
    println!("let {} {:?} {:?}", vec![y; 0usize].len(), y, o);
    if let Some(&x) = v.first() {
        out.push(vec![x; 0usize].len());
    }
    match v.get(2) {
        Some(&x) => out.push(vec![x; 3usize].len()),
        None => out.push(9),
    }
    println!("v2 {:?} {:?}", v, out);
    let w: Vec<usize> = v.iter().enumerate().map(|(i, &x)| i + vec![x; 0usize].len()).collect();
    println!("w {:?} {:?}", w, v);
    let pairs: Vec<(Option<i64>, u8)> = vec![(Some(1), 2), (None, 3)];
    let kept: Vec<(Option<i64>, u8)> = pairs
        .iter()
        .filter(|&&(a, b)| vec![a; 0usize].len() < usize::from(b))
        .cloned()
        .collect();
    println!("pairs {:?} {:?}", kept, pairs);
    let tuples: Vec<(i64, u8)> = vec![(1, 2), (3, 4)];
    let bumped: Vec<i64> = tuples
        .iter()
        .map(|&t: &(i64, u8)| {
            let mut own = t;
            own.0 += 10;
            own.0
        })
        .collect();
    println!("tuples {:?} {:?}", bumped, tuples);
    let points = vec![Point { a: 1, b: Some(2) }];
    let moved: Vec<i64> = points
        .iter()
        .map(|&p| {
            let mut q = p;
            q.a += 5;
            q.b = None;
            q.a + i64::from(q.b.unwrap_or(0))
        })
        .collect();
    println!("points {:?} {:?}", moved, points);
    let dropped: Vec<usize> = points.iter().map(|&p: &Point| vec![p; 0usize].len()).collect();
    println!("points {:?} {:?}", dropped, points);
    let g: Vec<Option<u8>> = vec![Some(1u8), None];
    let h: Vec<Option<u8>> = g.iter().map(|&x: &Option<u8>| Some(x).unwrap_or(x)).collect();
    println!("h {:?} {:?}", g, h);
    let picked = Some(3i64);
    println!("pick {} {:?}", pick(&picked), picked);
    let untyped: Vec<Option<i64>> = vec![Some(0i64)]
        .into_iter()
        .filter(|&x| Vec::<Option<i64>>::new() >= vec![x; 0usize])
        .collect();
    println!("untyped {:?}", untyped);
}
