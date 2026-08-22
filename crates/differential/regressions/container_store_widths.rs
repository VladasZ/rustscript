//! `push` keeps the usize width, so `usize::MAX` prints back as itself. Seed 20675004001.

fn opaque(v: u64) -> u64 {
    v
}

fn main() {
    let mut values: Vec<usize> = Vec::new();
    values.push((opaque(18446744073709551614) as usize).saturating_add(opaque(10217709742821619372) as usize));
    values.push(opaque(2) as usize);
    println!("{values:?}");

    // u64 values past `i64::MAX` sort by value
    let mut big: Vec<u64> = Vec::new();
    big.push(opaque(18446744073709551615));
    big.push(opaque(18446744073709551614));
    big.sort();
    println!("{big:?}");

    let mut map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    map.insert(String::from("big"), opaque(18446744073709551613));
    println!("{:?}", map.get("big"));
}
