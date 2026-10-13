// A bare literal handed to `get_or_insert` or `replace` is a payload, so it has the width
// of the payload. `Vec::as_array` has a known type, so the literals after it have one too.

fn none() -> Option<usize> {
    Vec::<i16>::new().as_array::<2>().map(|a| a.len())
}

fn main() {
    let mut c: Option<usize> = None;
    *c.get_or_insert(7) += 1;
    println!("{c:?}");
    let mut e: Option<usize> = None;
    let g = e.get_or_insert_with(|| 9);
    *g += 1;
    println!("{e:?}");
    let mut f: Option<u8> = None;
    *f.get_or_insert_with(|| 250) += 5;
    println!("{f:?}");
    let mut h: Option<u8> = Some(200);
    println!("{:?}", h.replace(250));
    if let Some(ref mut r) = h {
        *r += 5;
    }
    println!("{h:?} {}", !h.unwrap_or(0));

    let mut a = Vec::<i16>::new().as_array::<2>().map(|a| a.len());
    a.replace(0);
    if let Some(ref mut r) = a {
        *r += 1;
        println!("a {r:?}");
    }
    let mut d = vec![1i16, 2].as_array::<2>().map(|a| a.len());
    if let Some(ref mut r) = d {
        *r += 1;
    }
    *d.get_or_insert(7) += 1;
    let old = d.take();
    d.replace(3);
    while let Some(ref mut r) = d {
        *r += 4;
        if *r > 12 {
            break;
        }
    }
    println!("{a:?} {d:?} {old:?}");
    let mut arr = vec![3i16, 4].as_array::<2>().copied();
    if let Some(ref mut items) = arr {
        items[0] += 1;
    }
    let mut n = none();
    n.replace(1);
    let Some(ref mut r) = n else {
        return;
    };
    *r += 1;
    println!("{arr:?} {n:?}");
}
