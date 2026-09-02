//! Shared method cores for scalar receivers. A core works on plain Rust types, so the dispatch
//! layer only adapts values and the coverage harvest reads each core once. Nothing lazy or
//! stateful here.

use anyhow::{Result, anyhow, bail};

use super::numeric::IntWidth;

/// The cores monomorphize over this, the view is free.
pub(super) trait Args {
    /// what `Display` would print, missing arguments are empty
    fn text(&self, i: usize) -> String;
    fn int(&self, i: usize) -> Option<i64>;
    fn float(&self, i: usize) -> Option<f64>;
    /// the chars of a `['-', '_']` pattern, a char set splits on any member
    fn pattern_chars(&self, i: usize) -> Option<Vec<char>>;
}

fn int_arg(args: &impl Args, i: usize) -> Result<i64> {
    match args.int(i) {
        Some(n) => Ok(n),
        None => bail!("expected an integer argument"),
    }
}

/// A negative or oversized value can only be an interpreter bug, so error instead of wrapping.
fn usize_arg(args: &impl Args, i: usize) -> Result<usize> {
    let n = int_arg(args, i)?;
    usize::try_from(n).map_err(|_| anyhow!("`{n}` is not a valid count"))
}

/// Lengths fit in i64 on every platform we support.
pub(super) fn usize_i64(i: usize) -> i64 {
    i64::try_from(i).expect("value exceeds i64")
}

/// A length with its `usize` tag. Without the tag `!v.len()` is a small negative number instead
/// of a huge unsigned one.
pub(super) fn usize_value(i: usize) -> super::value::Value {
    super::value::Value::int_of_width(i128::from(usize_i64(i)), IntWidth::USize)
}

fn float_arg(args: &impl Args, i: usize) -> Result<f64> {
    match args.float(i) {
        Some(f) => Ok(f),
        None => bail!("expected a float argument"),
    }
}

mod json;
mod numbers;
mod regex;
mod scalars;
mod text;
mod time;

pub(crate) use json::*;
pub(crate) use numbers::*;
pub(crate) use regex::*;
pub(crate) use scalars::*;
pub(crate) use text::*;
pub(crate) use time::*;
