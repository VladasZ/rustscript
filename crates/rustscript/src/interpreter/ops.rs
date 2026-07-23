//! Binary and unary operator evaluation plus pattern binding for the
//! register machine. Split from `vm.rs`.

//! The register machine. Executes a compiled `Chunk` against one contiguous
//! register stack. Calls to user functions and closures push a frame record
//! and continue in the same instruction loop, so a script-level call costs no
//! native recursion, no allocation, and no register file copy beyond its
//! arguments. Anything else, methods and std or crate bridges, is delegated to
//! the existing dispatch on `Interp` with already evaluated values.

use std::cmp::Ordering;
use std::slice::from_ref;

use super::bytecode::{BinKind, PLit, PPat, UnKind};
use super::value::Value;
use anyhow::{Result, anyhow, bail};

// -- operators -------------------------------------------------------------

pub(super) fn apply_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    Ok(match op {
        Add | Sub | Mul | Div | Rem => return arith(op, l, r),
        Eq => Value::Bool(l.eq_value(r)),
        Ne => Value::Bool(!l.eq_value(r)),
        Lt => Value::Bool(compare_values(l, r)? == Ordering::Less),
        Le => Value::Bool(compare_values(l, r)? != Ordering::Greater),
        Gt => Value::Bool(compare_values(l, r)? == Ordering::Greater),
        Ge => Value::Bool(compare_values(l, r)? != Ordering::Less),
        BitAnd => int_bin(l, r, |a, b| a & b)?,
        BitOr => int_bin(l, r, |a, b| a | b)?,
        BitXor => int_bin(l, r, |a, b| a ^ b)?,
        Shl => int_bin(l, r, |a, b| a << b)?,
        Shr => int_bin(l, r, |a, b| a >> b)?,
    })
}

/// `apply_bin` with an integer literal right operand, with a fast integer path
/// that skips building a `Value` for the literal.
pub(super) fn apply_bin_imm(op: BinKind, l: &Value, imm: i64) -> Result<Value> {
    use BinKind::*;
    if let Value::Int(a) = l {
        let a = *a;
        return Ok(match op {
            Add => Value::Int(
                a.checked_add(imm)
                    .ok_or_else(|| anyhow!("attempt to add with overflow"))?,
            ),
            Sub => Value::Int(
                a.checked_sub(imm)
                    .ok_or_else(|| anyhow!("attempt to subtract with overflow"))?,
            ),
            Mul => Value::Int(
                a.checked_mul(imm)
                    .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?,
            ),
            Div => {
                if imm == 0 {
                    bail!("attempt to divide by zero");
                }
                Value::Int(
                    a.checked_div(imm)
                        .ok_or_else(|| anyhow!("attempt to divide with overflow"))?,
                )
            }
            Rem => {
                if imm == 0 {
                    bail!("attempt to calculate the remainder with a divisor of zero");
                }
                Value::Int(
                    a.checked_rem(imm).ok_or_else(|| {
                        anyhow!("attempt to calculate the remainder with overflow")
                    })?,
                )
            }
            Eq => Value::Bool(a == imm),
            Ne => Value::Bool(a != imm),
            Lt => Value::Bool(a < imm),
            Le => Value::Bool(a <= imm),
            Gt => Value::Bool(a > imm),
            Ge => Value::Bool(a >= imm),
            BitAnd => Value::Int(a & imm),
            BitOr => Value::Int(a | imm),
            BitXor => Value::Int(a ^ imm),
            Shl => Value::Int(a << imm),
            Shr => Value::Int(a >> imm),
        });
    }
    apply_bin(op, l, &Value::Int(imm))
}

/// Comparison result for the fused compare-and-branch ops.
pub(super) fn cmp_test(op: BinKind, l: &Value, r: &Value) -> Result<bool> {
    use BinKind::*;
    Ok(match op {
        Eq => l.eq_value(r),
        Ne => !l.eq_value(r),
        Lt => compare_values(l, r)? == Ordering::Less,
        Le => compare_values(l, r)? != Ordering::Greater,
        Gt => compare_values(l, r)? == Ordering::Greater,
        Ge => compare_values(l, r)? != Ordering::Less,
        _ => unreachable!("compare jump carries a non-comparison operator"),
    })
}

pub(super) fn cmp_test_imm(op: BinKind, l: &Value, imm: i64) -> Result<bool> {
    use BinKind::*;
    if let Value::Int(a) = l {
        let a = *a;
        return Ok(match op {
            Eq => a == imm,
            Ne => a != imm,
            Lt => a < imm,
            Le => a <= imm,
            Gt => a > imm,
            Ge => a >= imm,
            _ => unreachable!("compare jump carries a non-comparison operator"),
        });
    }
    cmp_test(op, l, &Value::Int(imm))
}

fn arith(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    if let (Add, Value::Str(a), Value::Str(b)) = (op, l, r) {
        let mut out = String::with_capacity(a.len() + b.len());
        out.push_str(a);
        out.push_str(b);
        return Ok(Value::str(out));
    }
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => {
            let (a, b) = (*a, *b);
            let result = match op {
                Add => a
                    .checked_add(b)
                    .ok_or_else(|| anyhow!("attempt to add with overflow"))?,
                Sub => a
                    .checked_sub(b)
                    .ok_or_else(|| anyhow!("attempt to subtract with overflow"))?,
                Mul => a
                    .checked_mul(b)
                    .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?,
                Div => {
                    if b == 0 {
                        bail!("attempt to divide by zero");
                    }
                    a.checked_div(b)
                        .ok_or_else(|| anyhow!("attempt to divide with overflow"))?
                }
                Rem => {
                    if b == 0 {
                        bail!("attempt to calculate the remainder with a divisor of zero");
                    }
                    a.checked_rem(b).ok_or_else(|| {
                        anyhow!("attempt to calculate the remainder with overflow")
                    })?
                }
                _ => unreachable!(),
            };
            Ok(Value::Int(result))
        }
        (a, b) => {
            let (x, y) = (to_float(a)?, to_float(b)?);
            Ok(Value::Float(match op {
                Add => x + y,
                Sub => x - y,
                Mul => x * y,
                Div => x / y,
                Rem => x % y,
                _ => unreachable!(),
            }))
        }
    }
}

fn int_bin(l: &Value, r: &Value, f: impl Fn(i64, i64) -> i64) -> Result<Value> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(f(*a, *b))),
        (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(f(*a as i64, *b as i64) != 0)),
        _ => bail!("bitwise operators need integers"),
    }
}

pub(super) fn compare_values(l: &Value, r: &Value) -> Result<Ordering> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a
            .partial_cmp(b)
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Int(a), Value::Float(b)) => (*a as f64)
            .partial_cmp(b)
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Float(a), Value::Int(b)) => a
            .partial_cmp(&(*b as f64))
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Str(a), Value::Str(b)) => a.as_str().cmp(b.as_str()),
        (Value::Char(a), Value::Char(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (a, b) => bail!("cannot compare {} and {}", a.type_name(), b.type_name()),
    })
}

fn to_float(v: &Value) -> Result<f64> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => bail!("expected a number, got {}", other.type_name()),
    }
}

pub(super) fn apply_un(op: UnKind, v: &Value) -> Result<Value> {
    Ok(match (op, v) {
        (UnKind::Neg, Value::Int(i)) => Value::Int(-*i),
        (UnKind::Neg, Value::Float(f)) => Value::Float(-*f),
        (UnKind::Not, Value::Bool(b)) => Value::Bool(!*b),
        (UnKind::Not, Value::Int(i)) => Value::Int(!*i),
        (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
    })
}

// -- patterns --------------------------------------------------------------

/// Match `pat` against `val`, calling `define` for each bound name. Returns
/// false without fully binding when the pattern does not match.
pub(super) fn try_bind(pat: &PPat, val: &Value, define: &mut dyn FnMut(&str, Value)) -> bool {
    match pat {
        PPat::Wild | PPat::Rest => true,
        PPat::Ident { name, sub } => {
            if let Some(subpattern) = sub
                && !try_bind(subpattern, val, define)
            {
                return false;
            }
            define(name, val.clone());
            true
        }
        PPat::Lit(literal) => literal_matches(literal, val),
        PPat::Tuple(patterns) => match val {
            Value::Tuple(items) => bind_seq(patterns, &items.borrow(), define),
            Value::Unit if patterns.is_empty() => true,
            _ => false,
        },
        PPat::TupleStruct { name, elems } => match val {
            Value::Enum { variant, data, .. } => {
                name.as_deref() == Some(&**variant) && bind_seq(elems, data, define)
            }
            Value::Struct(structure) => bind_seq(elems, &structure.values.borrow(), define),
            // A json string is a plain Str here, so a serde accessor like
            // as_str hands back the string itself as an already unwrapped Some,
            // the same model the Option methods on a Str follow. Matching a
            // bare value against Some(x) does not type check in real Rust, so
            // the script can only mean that pre-unwrapped Some. Unit is left
            // out because it is also this interpreter's filler for a missing
            // value.
            Value::Unit => false,
            other => name.as_deref() == Some("Some") && bind_seq(elems, from_ref(other), define),
        },
        PPat::Path { name } => match val {
            Value::Enum { variant, .. } => name.as_deref() == Some(&**variant),
            _ => false,
        },
        PPat::Struct { name, fields } => {
            let Value::Struct(structure) = val else {
                return false;
            };
            if let Some(pattern_name) = name
                && pattern_name != super::resolver::bare(structure.name())
            {
                return false;
            }
            for (field, pattern) in fields {
                match structure.get(field) {
                    Some(value) if try_bind(pattern, &value, define) => {}
                    _ => return false,
                }
            }
            true
        }
        PPat::Or(patterns) => patterns
            .iter()
            .any(|pattern| try_bind(pattern, val, define)),
        PPat::Slice(patterns) => match val {
            Value::Vec(items) => bind_seq(patterns, &items.borrow(), define),
            _ => false,
        },
        PPat::Range { lo, hi, inclusive } => {
            range_matches(lo.as_ref(), hi.as_ref(), *inclusive, |l| {
                endpoint_cmp(l, val)
            })
        }
        PPat::Unsupported => false,
    }
}

/// Order a range endpoint against a value of the same type. `None` for a type
/// mismatch, which makes the range not match.
fn endpoint_cmp(literal: &PLit, value: &Value) -> Option<Ordering> {
    match (literal, value) {
        (PLit::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (PLit::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (PLit::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Shared range test, parameterized over the engine's endpoint comparison.
/// `cmp` orders an endpoint literal against the matched value.
pub(super) fn range_matches<L>(
    lo: Option<&L>,
    hi: Option<&L>,
    inclusive: bool,
    cmp: impl Fn(&L) -> Option<Ordering>,
) -> bool {
    if let Some(l) = lo {
        match cmp(l) {
            Some(Ordering::Less | Ordering::Equal) => {}
            _ => return false,
        }
    }
    if let Some(h) = hi {
        match cmp(h) {
            Some(Ordering::Greater) => {}
            Some(Ordering::Equal) if inclusive => {}
            _ => return false,
        }
    }
    true
}

fn bind_seq(patterns: &[PPat], vals: &[Value], define: &mut dyn FnMut(&str, Value)) -> bool {
    if patterns.iter().any(|pattern| matches!(pattern, PPat::Rest)) {
        let head_len = patterns
            .iter()
            .take_while(|pattern| !matches!(pattern, PPat::Rest))
            .count();
        for (pattern, value) in patterns.iter().take(head_len).zip(vals.iter()) {
            if !try_bind(pattern, value, define) {
                return false;
            }
        }
        for (pattern, value) in patterns.iter().skip(head_len + 1).zip(vals.iter().rev()) {
            if !try_bind(pattern, value, define) {
                return false;
            }
        }
        return true;
    }
    patterns.len() == vals.len()
        && patterns
            .iter()
            .zip(vals.iter())
            .all(|(pattern, value)| try_bind(pattern, value, define))
}

fn literal_matches(literal: &PLit, value: &Value) -> bool {
    match (literal, value) {
        (PLit::Int(left), Value::Int(right)) => left == right,
        (PLit::Float(left), Value::Float(right)) => left == right,
        (PLit::Bool(left), Value::Bool(right)) => left == right,
        (PLit::Str(left), Value::Str(right)) => left == right.as_str(),
        (PLit::Char(left), Value::Char(right)) => left == right,
        _ => false,
    }
}
