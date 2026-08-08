#!/usr/bin/env rust

//! `unwrap_or_default` builds `T::default()`, and `T` is the payload of the
//! Option or Result it is called on. The method takes no turbofish, so the
//! type has to come from wherever the source states it: the binding's
//! annotation, the argument of the call that built the Option, a `None::<T>`,
//! or the shape of the chain itself. Without that the default was an empty
//! string whatever the real type was, which surfaced further along as a
//! confusing "cannot cast String to integer".

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
}
