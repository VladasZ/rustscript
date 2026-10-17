#!/usr/bin/env rust

use std::fs::{File, remove_file};
use std::io::{BufRead, BufReader, Write};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_lines_adapters.txt");
    File::create(&path)?.write_all(b"one\ntwo\nthree\n")?;
    let open = || -> anyhow::Result<BufReader<File>> { Ok(BufReader::new(File::open(&path)?)) };

    let upper: Vec<String> = open()?
        .lines()
        .map(|line| line.map(|l| l.to_uppercase()))
        .collect::<Result<_, _>>()?;
    println!("map then collect into Result {upper:?}");

    let lens: Vec<usize> = open()?
        .lines()
        .map_while(Result::ok)
        .map(|l| l.len())
        .collect();
    println!("map_while lens {lens:?}");

    let count = open()?.lines().count();
    let long = open()?
        .lines()
        .filter(|line| line.as_ref().is_ok_and(|l| l.len() > 3))
        .count();
    println!("count {count}, longer than 3 {long}");

    for (i, line) in open()?.lines().enumerate().skip(1) {
        println!("line {i} {}", line?);
    }

    remove_file(&path)?;
    Ok(())
}
