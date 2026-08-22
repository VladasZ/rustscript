#!/usr/bin/env rust


fn main() {
    let present = Some(5i64);
    let absent: Option<i64> = None;
    println!("present: {:?}", present.into_iter().collect::<Vec<i64>>());
    println!("absent:  {:?}", absent.into_iter().collect::<Vec<i64>>());

    let word = Some(String::from("rust"));
    println!("word:    {:?}", word.into_iter().collect::<Vec<String>>());

    // The shape the differential generator writes for `opt_to_vec`.
    let checked = 200u8.checked_add(100);
    println!("checked: {:?}", checked.into_iter().collect::<Vec<u8>>());
    let fine = 200u8.checked_add(50);
    println!("fine:    {:?}", fine.into_iter().collect::<Vec<u8>>());
}
