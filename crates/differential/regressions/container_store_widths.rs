//! A value stored into a container keeps its real width. The campaign found
//! `push` flattening a usize argument to an i64 image, so a saturated
//! usize::MAX printed back as i64::MAX. From seed 20675004001.

fn opaque(v: u64) -> u64 {
    v
}

fn main() {
    let mut values: Vec<usize> = Vec::new();
    values.push((opaque(18446744073709551614) as usize).saturating_add(opaque(10217709742821619372) as usize));
    values.push(opaque(2) as usize);
    println!("{values:?}");

    // Two u64 values past i64::MAX still sort by value.
    let mut big: Vec<u64> = Vec::new();
    big.push(opaque(18446744073709551615));
    big.push(opaque(18446744073709551614));
    big.sort();
    println!("{big:?}");

    let mut map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    map.insert(String::from("big"), opaque(18446744073709551613));
    println!("{:?}", map.get("big"));
}
