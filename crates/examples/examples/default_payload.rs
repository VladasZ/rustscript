#!/usr/bin/env rust

//! `unwrap_or_default` builds `T::default()`, and `T` is the payload of the
//! Option or Result it is called on. The method takes no turbofish, so the
//! type has to come from wherever the source states it: the binding's
//! annotation, the argument of the call that built the Option, a `None::<T>`,
//! a `collect` turbofish on the chain that built the container, or the shape
//! of the chain itself. Without that the default was an empty string whatever
//! the real type was, which surfaced further along as a confusing "cannot
//! cast String to integer".

use std::collections::HashMap;

fn main() {
    // Runtime false, so nothing here folds at compile time and every Option
    // below really is empty when its default is taken.
    let flag = std::env::args().count() > 1000;

    // The payload named by the binding's own annotation.
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

    // A container answers with the default it has, whatever it wraps.
    let lists: Vec<Vec<u8>> = Vec::new();
    let missing_vec: Option<Vec<u8>> = lists.first().cloned();
    println!("local vec:    {:?}", missing_vec.unwrap_or_default());

    let nested: Vec<Option<f64>> = Vec::new();
    let missing_opt: Option<Option<f64>> = nested.first().copied();
    println!("local option: {:?}", missing_opt.unwrap_or_default());

    // A Result defaults from its Ok payload the same way.
    let failed: Result<i16, String> = if flag { Ok(1) } else { Err("nope".to_string()) };
    println!("result i16:   {:?}", failed.unwrap_or_default());

    // A present value never reaches the default at all.
    let present: Option<f64> = [2.5f64].first().copied();
    println!("present:      {:?}", present.unwrap_or_default());

    // The long standing shape this method is mostly used for still works.
    let unset = std::env::var("RUSTSCRIPT_DEFINITELY_UNSET").ok();
    println!("env var:      {:?}", unset.unwrap_or_default());

    // A `collect` turbofish states the element type, an Option here, so a
    // missing element defaults to None.
    let collected = [Some(1.0f32)]
        .into_iter()
        .collect::<Vec<Option<f32>>>()
        .get(2)
        .copied()
        .unwrap_or_default();
    println!("collected:    {collected:?}");

    // A map built by a `collect` turbofish states its value type there, so a
    // missed `get` defaults to the empty vec, not the empty string.
    let missed = vec![String::from("a"), String::from("b")]
        .into_iter()
        .map(|key: String| (key.clone(), vec![1i64, 2i64]))
        .collect::<HashMap<String, Vec<i64>>>()
        .get(&String::from("missing"))
        .cloned()
        .unwrap_or_default();
    println!("map miss:     {missed:?}");

    // The closure's own collect turbofish is the only place this element
    // type is written down, and `min` of no elements defaults from it.
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

    // The ASCII case methods keep their receiver's type, so `then_some` of a
    // lowered char states a char payload and its default is the nul char, not
    // the empty string.
    let initial: char = 'R';
    let lowered = flag.then_some(initial.to_ascii_lowercase());
    println!("then_some:    {:?}", lowered.unwrap_or_default());

    // The annotated closure param is where this char is written down, kept
    // through the ASCII case method, and `max` of no elements defaults from
    // it.
    let no_letters: Vec<char> = Vec::new();
    let largest = no_letters
        .into_iter()
        .map(|letter: char| letter.to_ascii_uppercase())
        .max()
        .unwrap_or_default();
    println!("max miss:     {largest:?}");

    // An if-else states its type through either branch, so a `then_some` of
    // one holds a char and defaults to the nul char.
    let fallback: char = 'x';
    let chosen = flag.then_some(if flag { '9' } else { fallback });
    println!("if branch:    {:?}", chosen.unwrap_or_default());

    // The arithmetic methods answer in their receiver's width, so the u8
    // param this closure multiplies is where the element type is written
    // down and `max` of no elements defaults to 0u8.
    let no_bytes: Vec<u8> = Vec::new();
    let widest = no_bytes
        .into_iter()
        .map(|value: u8| value.saturating_mul(3))
        .max()
        .unwrap_or_default();
    println!("arith miss:   {widest:?}");
}
