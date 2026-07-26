#!/usr/bin/env rust

// extend takes anything iterable, not only another Vec. A lazy argument such as
// `.iter().map(..)` used to append nothing at all and still report success, so
// the caller saw a short vec and no error.

fn argv(devices: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = vec!["readlink", "-f"];
    out.extend(devices.iter().map(String::as_str));
    out
}

fn main() {
    let devices = vec!["/dev/ttyUSB0".to_string(), "/dev/ttyUSB2".to_string()];
    println!("{:?}", argv(&devices));
    println!("{:?}", argv(&[]));

    let mut cloned: Vec<String> = vec!["head".to_string()];
    cloned.extend(devices.iter().cloned());
    println!("{cloned:?}");

    let mut filtered: Vec<String> = Vec::new();
    filtered.extend(devices.iter().filter(|d| d.ends_with('2')).cloned());
    println!("{filtered:?}");

    let mut from_vec: Vec<i32> = vec![1];
    from_vec.extend(vec![2, 3]);
    println!("{from_vec:?}");

    let mut counted: Vec<usize> = Vec::new();
    counted.extend(devices.iter().map(String::len));
    println!("{counted:?}");
}
