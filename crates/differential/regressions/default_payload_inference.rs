// The `unwrap_or_default` payload shapes found on 2026-08-08, seeds
// 20673101201, 20673004841, 20673200427, 20673207491 and 20673005126. The
// default once fell back to an empty string for all of them.

use std::collections::HashMap;

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn diff_opaque_i64(value: i64) -> i64 {
    value
}

fn diff_opaque_f32(value: f32) -> f32 {
    value
}

fn main() {
    let flag = diff_opaque_u64(0) > 1000;

    // A vec literal's elements.
    println!("{:?}", vec![(diff_opaque_i64(113) as i8), (diff_opaque_i64(-127) as i8)].get((diff_opaque_u64(3) as usize)).cloned().unwrap_or_default());

    // A `Vec::<T>::new()` turbofish, one more unwrap peels one more layer.
    println!("{:?}", Some(Vec::<Vec<u8>>::new()).unwrap_or_default().get((diff_opaque_u64(2) as usize)).cloned().unwrap_or_default());

    // Iterator reductions.
    println!("{:?}", Some(Vec::<Vec<String>>::new()).unwrap_or(vec![vec![String::from("a,b,c"), String::from("5")]]).iter().min().cloned().unwrap_or_default());

    // Nested options.
    println!("{:?}", vec![Some(None::<i64>), None::<Option<i64>>].get((diff_opaque_u64(3) as usize)).cloned().unwrap_or_default().unwrap_or_default());

    // An if else branch.
    let value: i16 = (if flag { Some((diff_opaque_i64(-13504) as i16)) } else { None::<i16> }).unwrap_or_default();
    println!("{:?}", value);

    // A declared `Vec<T>` local.
    let mut declared: Vec<u16> = vec![(diff_opaque_u64(7) as u16)];
    declared.push((diff_opaque_u64(9) as u16));
    println!("{:?}", declared.get((diff_opaque_u64(5) as usize)).copied().unwrap_or_default());

    // `checked_shl`.
    println!("{:?}", (diff_opaque_u64(200) as u8).checked_shl((diff_opaque_u64(40) as u32)).unwrap_or_default());

    // A declared return type. From seed 778227.
    println!("{:?}", flag.then_some(diff_opaque_f32((-0.0f32))).unwrap_or_default());

    // `unwrap_or` peels one layer. From seed 426738.
    println!("{:?}", vec![Some((diff_opaque_u64(65534) as u16)), None::<u16>].last().cloned().unwrap_or((diff_opaque_u64(65534) as u16).checked_mul((diff_opaque_u64(52843) as u16))).unwrap_or_default());

    // A `parse` turbofish through `ok`. From seed 20673218959.
    println!("{:?}", (diff_opaque_f32(1.5f32) as f64) / String::from("  ").parse::<f64>().ok().unwrap_or_default());

    // An if else vec literal. From seed 20673109586.
    let letter = 'x';
    println!("{:?}", (if flag { vec![letter, 'a', letter] } else { vec![letter, letter, ' '] }).get((diff_opaque_u64(3) as usize)).cloned().unwrap_or_default());

    // Arithmetic keeps its operands' type. From seed 20673005366.
    println!("{:?}", flag.then_some(((diff_opaque_i64(127) as i8) / (diff_opaque_i64(125) as i8))).unwrap_or_default());

    // An inner unwrap's payload. From seed 20673118405.
    println!("{:?}", flag.then_some(Some('\n').unwrap_or_default()).unwrap_or_default());

    // `String::from` as a vec element. From seed 20673204730.
    println!("{:?}", Some(vec![Some(String::from("5 "))]).unwrap_or_default().get((diff_opaque_u64(2) as usize)).cloned().unwrap_or_default());

    // A scalar annotated local. From seed 20673211807.
    let scalar_local: u16 = (diff_opaque_u64(9) as u16);
    println!("{:?}", scalar_local.checked_rem((diff_opaque_u64(0) as u16)).unwrap_or_default());

    // `clone` keeps the type. From seed 20673216305.
    let cloned_vec: Vec<i8> = vec![(diff_opaque_i64(-127) as i8)];
    println!("{:?}", vec![cloned_vec.clone()].get((diff_opaque_u64(2) as usize)).cloned().unwrap_or_default());

    // A `collect` turbofish of Options. From seed 20675204441.
    println!("{:?}", Some(Some(diff_opaque_f32(1.0f32))).into_iter().collect::<Vec<Option<f32>>>().get((diff_opaque_u64(2) as usize)).cloned().unwrap_or_default());

    // `to_digit` is `Option<u32>`. From seed 20677208695.
    println!("{:?}", 'é'.to_digit(10).unwrap_or_default());

    // `remove` through a block tail. From seed 20677214481.
    let mut diff_owned = {
        let mut diff_map: HashMap<String, i64> = HashMap::new();
        diff_map.insert(String::from("  padded  "), diff_opaque_i64(-3));
        diff_map
    };
    println!("{:?}", diff_owned.remove(&String::from("true")).unwrap_or_default());
}
