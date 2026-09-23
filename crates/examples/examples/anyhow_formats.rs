#!/usr/bin/env rust


use anyhow::{Context, Result, anyhow, bail, ensure};
use std::fs;

fn read(p: &str) -> Result<String> {
    let s = fs::read_to_string(p).with_context(|| format!("reading {p}"))?;
    Ok(s)
}
fn layered() -> Result<()> {
    read("/nope/x.txt").context("loading config")?;
    Ok(())
}
fn parse(s: &str) -> Result<i32> {
    let n: i32 = s.trim().parse()?;
    ensure!(n > 0, "n must be positive, got {n}");
    if n > 100 {
        bail!("too big: {n}");
    }
    Ok(n)
}
fn first(v: &[i32]) -> Result<i32> {
    v.first().copied().context("empty list")
}
fn main() {
    for r in [
        layered().map(|()| 0),
        parse("x"),
        parse("-1"),
        parse("500"),
        parse("7"),
        first(&[]),
        Err(anyhow!("plain {}", 1)),
    ] {
        match r {
            Ok(v) => println!("ok {v}"),
            Err(e) => {
                println!("display: {e}");
                println!("alt: {e:#}");
                println!("debug: {e:?}");
                println!("width: [{e:>20}]");
                println!("---");
            }
        }
    }
    let e = anyhow!("x");
    println!("{e} {:?} {}", parse("2"), e.to_string().len());
    println!("{:?}", layered().map_err(|e| format!("{e:#}")));
}
