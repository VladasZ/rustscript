#!/usr/bin/env rust

//! `unwrap_or_default` takes no turbofish, so its type comes from wherever the source states it.
//! Otherwise the default is an empty string and fails later with "cannot cast String to integer".

use std::collections::{HashMap, HashSet};

fn main() {
    // runtime false, so nothing folds at compile time
    let flag = std::env::args().count() > 1000;

    // from the binding annotation
    let ints: Vec<u64> = Vec::new();
    let missing_int: Option<u64> = ints.first().copied();
    println!("local u64:    {:?}", missing_int.unwrap_or_default());

    let floats: Vec<f32> = Vec::new();
    let missing_float: Option<f32> = floats.first().copied();
    println!("local f32:    {:?}", missing_float.unwrap_or_default());

    let flags: Vec<bool> = Vec::new();
    let missing_bool: Option<bool> = flags.first().copied();
    println!("local bool:   {:?}", missing_bool.unwrap_or_default());

    let words: Vec<String> = Vec::new();
    let missing_text: Option<String> = words.first().cloned();
    println!("local string: {:?}", missing_text.unwrap_or_default());

    let lists: Vec<Vec<u8>> = Vec::new();
    let missing_vec: Option<Vec<u8>> = lists.first().cloned();
    println!("local vec:    {:?}", missing_vec.unwrap_or_default());

    let nested: Vec<Option<f64>> = Vec::new();
    let missing_opt: Option<Option<f64>> = nested.first().copied();
    println!("local option: {:?}", missing_opt.unwrap_or_default());

    // a Result defaults from its Ok payload
    let failed: Result<i16, String> = if flag { Ok(1) } else { Err("nope".to_string()) };
    println!("result i16:   {:?}", failed.unwrap_or_default());

    let present: Option<f64> = [2.5f64].first().copied();
    println!("present:      {:?}", present.unwrap_or_default());

    let unset = std::env::var("RUSTSCRIPT_DEFINITELY_UNSET").ok();
    println!("env var:      {:?}", unset.unwrap_or_default());

    // from a `collect` turbofish
    let collected = [Some(1.0f32)]
        .into_iter()
        .collect::<Vec<Option<f32>>>()
        .get(2)
        .copied()
        .unwrap_or_default();
    println!("collected:    {collected:?}");

    // a missed `get` defaults to the empty vec, not the empty string
    let missed = vec![String::from("a"), String::from("b")]
        .into_iter()
        .map(|key: String| (key.clone(), vec![1i64, 2i64]))
        .collect::<HashMap<String, Vec<i64>>>()
        .get(&String::from("missing"))
        .cloned()
        .unwrap_or_default();
    println!("map miss:     {missed:?}");

    // the closure's collect turbofish is the only place the element type is written
    let empty: Vec<char> = Vec::new();
    let smallest = empty
        .into_iter()
        .map(|letter: char| {
            vec![letter.is_uppercase()]
                .into_iter()
                .rev()
                .collect::<Vec<bool>>()
        })
        .min()
        .unwrap_or_default();
    println!("min miss:     {smallest:?}");

    // the ASCII case methods keep the char type, so the default is the nul char
    let initial: char = 'R';
    let lowered = flag.then_some(initial.to_ascii_lowercase());
    println!("then_some:    {:?}", lowered.unwrap_or_default());

    // from the annotated closure param
    let no_letters: Vec<char> = Vec::new();
    let largest = no_letters
        .into_iter()
        .map(|letter: char| letter.to_ascii_uppercase())
        .max()
        .unwrap_or_default();
    println!("max miss:     {largest:?}");

    // an if else states its type through either branch
    let fallback: char = 'x';
    let chosen = flag.then_some(if flag { '9' } else { fallback });
    println!("if branch:    {:?}", chosen.unwrap_or_default());

    // arithmetic keeps the width of the u8 param, so the default is 0u8
    let no_bytes: Vec<u8> = Vec::new();
    let widest = no_bytes
        .into_iter()
        .map(|value: u8| value.saturating_mul(3))
        .max()
        .unwrap_or_default();
    println!("arith miss:   {widest:?}");

    chain_shapes();
}

/// Only the chain itself states the payload type here.
fn chain_shapes() {
    // `position` and `str::find` are usize whatever the items are
    let empty_floats: Vec<f32> = Vec::new();
    let at = empty_floats
        .iter()
        .position(|value| *value > 0.5)
        .unwrap_or_default();
    println!("position:     {at:?}");
    println!("str find:     {:?}", "rust".find('z').unwrap_or_default());

    // the element type survives the middle of a chain
    let no_ints: Vec<i32> = Vec::new();
    let through = no_ints
        .iter()
        .copied()
        .rev()
        .filter(|value| *value > 0)
        .take(3)
        .skip(1)
        .take_while(|value| *value < 9)
        .min()
        .unwrap_or_default();
    println!("chain miss:   {through:?}");

    // an unannotated closure param takes the item type from the chain
    let mapped = no_ints
        .iter()
        .map(|value| value + 1)
        .min()
        .unwrap_or_default();
    println!("closure item: {mapped:?}");

    // `find`, `next` and `reduce` on an iterator return an item
    println!(
        "iter find:    {:?}",
        no_ints
            .iter()
            .copied()
            .find(|value| *value > 0)
            .unwrap_or_default()
    );
    println!(
        "iter next:    {:?}",
        no_ints.iter().copied().next().unwrap_or_default()
    );
    println!(
        "reduce:       {:?}",
        no_ints
            .iter()
            .copied()
            .reduce(|left, right| left + right)
            .unwrap_or_default()
    );
    println!(
        "min_by_key:   {:?}",
        no_ints
            .iter()
            .copied()
            .min_by_key(|value| *value)
            .unwrap_or_default()
    );

    // chars and bytes of a string
    let blank = String::new();
    println!(
        "chars next:   {:?}",
        blank.chars().next().unwrap_or_default()
    );
    println!(
        "bytes max:    {:?}",
        blank.bytes().max().unwrap_or_default()
    );

    // a range and a set
    println!("range min:    {:?}", (0i16..0i16).min().unwrap_or_default());
    let no_keys: HashSet<u32> = HashSet::new();
    println!(
        "set min:      {:?}",
        no_keys.iter().copied().min().unwrap_or_default()
    );

    // map values
    let scores: HashMap<String, i8> = HashMap::new();
    println!(
        "values min:   {:?}",
        scores.values().copied().min().unwrap_or_default()
    );

    // `Option::map` and `and_then` read their param as the receiver payload
    let absent: Option<u8> = None;
    println!(
        "opt map:      {:?}",
        absent
            .map(|value| value.saturating_mul(3))
            .unwrap_or_default()
    );
    println!(
        "opt and_then: {:?}",
        absent
            .and_then(|value| value.checked_add(1))
            .unwrap_or_default()
    );

    // `filter_map` yields the payload of the closure's Option
    let no_bytes: Vec<u8> = Vec::new();
    let picked = no_bytes
        .iter()
        .copied()
        .filter_map(|value| value.checked_mul(2))
        .min()
        .unwrap_or_default();
    println!("filter_map:   {picked:?}");
}
