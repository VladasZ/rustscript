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
use super::numeric::{IntWidth, int_arith, int_bit, int_neg, int_not, int_shift, unify};
use super::value::Value;
use anyhow::{Result, anyhow, bail};

/// The u64 view of a width-tagged value when its width is 64-bit unsigned.
/// u64 and usize dominate real scripts, sizes, counts, and hashes, so they
/// get the same kind of inline fast path plain i64 has.
#[inline(always)]
fn as_u64(v: &Value) -> Option<(u64, IntWidth)> {
    match v {
        Value::IntW(stored, w @ (IntWidth::U64 | IntWidth::USize)) => Some((*stored as u64, *w)),
        _ => None,
    }
}

#[inline(always)]
fn u64_arith(op: BinKind, a: u64, b: u64, w: IntWidth) -> Result<Value> {
    use BinKind::*;
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
            a / b
        }
        Rem => {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            a % b
        }
        _ => unreachable!(),
    };
    Ok(Value::IntW(result as i64, w))
}

// -- operators -------------------------------------------------------------

pub(super) fn apply_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    Ok(match op {
        Add | Sub | Mul | Div | Rem => return arith(op, l, r),
        Eq => Value::Bool(l.eq_value(r)),
        Ne => Value::Bool(!l.eq_value(r)),
        Lt => Value::Bool(partial_compare(l, r)? == Some(Ordering::Less)),
        Le => Value::Bool(matches!(
            partial_compare(l, r)?,
            Some(Ordering::Less | Ordering::Equal)
        )),
        Gt => Value::Bool(partial_compare(l, r)? == Some(Ordering::Greater)),
        Ge => Value::Bool(matches!(
            partial_compare(l, r)?,
            Some(Ordering::Greater | Ordering::Equal)
        )),
        BitAnd | BitOr | BitXor => bit_bin(op, l, r)?,
        Shl | Shr => shift_bin(op, l, r)?,
    })
}

/// `apply_bin` with an integer literal right operand, with a fast integer path
/// that skips building a `Value` for the literal. Each operator keeps its own
/// arm so the dispatch stays a single match.
#[inline]
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
            Shl | Shr => shift_bin(op, l, &Value::Int(imm))?,
        });
    }
    // A literal next to a u64 or usize value is that type itself, so it is
    // never negative in a program that passed the real type checker.
    if let Some((a, w)) = as_u64(l)
        && imm >= 0
    {
        let b = imm as u64;
        return Ok(match op {
            Add | Sub | Mul | Div | Rem => u64_arith(op, a, b, w)?,
            Eq => Value::Bool(a == b),
            Ne => Value::Bool(a != b),
            Lt => Value::Bool(a < b),
            Le => Value::Bool(a <= b),
            Gt => Value::Bool(a > b),
            Ge => Value::Bool(a >= b),
            BitAnd => Value::IntW((a & b) as i64, w),
            BitOr => Value::IntW((a | b) as i64, w),
            BitXor => Value::IntW((a ^ b) as i64, w),
            Shl | Shr => shift_bin(op, l, &Value::Int(imm))?,
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
        Lt => partial_compare(l, r)? == Some(Ordering::Less),
        Le => matches!(
            partial_compare(l, r)?,
            Some(Ordering::Less | Ordering::Equal)
        ),
        Gt => partial_compare(l, r)? == Some(Ordering::Greater),
        Ge => matches!(
            partial_compare(l, r)?,
            Some(Ordering::Greater | Ordering::Equal)
        ),
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
    if let Some((a, _)) = as_u64(l)
        && imm >= 0
    {
        let b = imm as u64;
        return Ok(match op {
            Eq => a == b,
            Ne => a != b,
            Lt => a < b,
            Le => a <= b,
            Gt => a > b,
            Ge => a >= b,
            _ => unreachable!("compare jump carries a non-comparison operator"),
        });
    }
    cmp_test(op, l, &Value::Int(imm))
}

/// The hot i64 case stays inline in the dispatch loop with one match per
/// operator, and everything else, strings, width-tagged integers, and
/// floats, lives in the outlined general path.
#[inline]
fn arith(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    if let (Value::Int(a), Value::Int(b)) = (l, r) {
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
                a.checked_rem(b)
                    .ok_or_else(|| anyhow!("attempt to calculate the remainder with overflow"))?
            }
            _ => unreachable!(),
        };
        return Ok(Value::Int(result));
    }
    if let (Some((a, w)), Some((b, _))) = (as_u64(l), as_u64(r)) {
        return u64_arith(op, a, b, w);
    }
    arith_general(op, l, r)
}

#[inline(never)]
fn arith_general(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    if let (Add, Value::Str(a), Value::Str(b)) = (op, l, r) {
        let mut out = String::with_capacity(a.len() + b.len());
        out.push_str(a);
        out.push_str(b);
        return Ok(Value::str(out));
    }
    if let (Some((a, wa)), Some((b, wb))) = (l.int_parts(), r.int_parts()) {
        let width = unify(wa, wb)?;
        return Ok(Value::int_of_width(int_arith(op, width, a, b)?, width));
    }
    match float_pair(l, r)? {
        FloatPair::F64(x, y) => Ok(Value::Float(match op {
            Add => x + y,
            Sub => x - y,
            Mul => x * y,
            Div => x / y,
            Rem => x % y,
            _ => unreachable!(),
        })),
        FloatPair::F32(x, y) => Ok(Value::F32(match op {
            Add => x + y,
            Sub => x - y,
            Mul => x * y,
            Div => x / y,
            Rem => x % y,
            _ => unreachable!(),
        })),
    }
}

/// The two sides of a float op at the width they compute in. An untagged f64
/// next to an f32 is a bare literal that is f32 in the source types, so it
/// rounds to f32 and the op runs at f32 precision.
enum FloatPair {
    F64(f64, f64),
    F32(f32, f32),
}

fn float_pair(l: &Value, r: &Value) -> Result<FloatPair> {
    Ok(match (l, r) {
        (Value::F32(a), Value::F32(b)) => FloatPair::F32(*a, *b),
        (Value::F32(a), Value::Float(b)) => FloatPair::F32(*a, *b as f32),
        (Value::Float(a), Value::F32(b)) => FloatPair::F32(*a as f32, *b),
        (a, b) => FloatPair::F64(to_float(a)?, to_float(b)?),
    })
}

fn bit_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    if let (Value::Int(a), Value::Int(b)) = (l, r) {
        return Ok(Value::Int(match op {
            BinKind::BitAnd => a & b,
            BinKind::BitOr => a | b,
            _ => a ^ b,
        }));
    }
    if let (Value::Bool(a), Value::Bool(b)) = (l, r) {
        let (a, b) = (*a as i64, *b as i64);
        let bits = match op {
            BinKind::BitAnd => a & b,
            BinKind::BitOr => a | b,
            _ => a ^ b,
        };
        return Ok(Value::Bool(bits != 0));
    }
    if let (Some((a, wa)), Some((b, wb))) = (l.int_parts(), r.int_parts()) {
        let width = unify(wa, wb)?;
        return Ok(Value::int_of_width(int_bit(op, a, b)?, width));
    }
    bail!("bitwise operators need integers")
}

/// `<<` and `>>`. The amount side keeps its own width and only supplies the
/// count, the result has the shifted side's width.
fn shift_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    let (Some((a, wa)), Some((b, _))) = (l.int_parts(), r.int_parts()) else {
        bail!("shift operators need integers");
    };
    Ok(Value::int_of_width(int_shift(op, wa, a, b)?, wa))
}

pub(super) fn compare_values(l: &Value, r: &Value) -> Result<Ordering> {
    partial_compare(l, r)?.ok_or_else(|| anyhow!("cannot order NaN"))
}

/// PartialOrd semantics: a NaN operand compares as `None`, which makes every
/// ordered comparison operator false, exactly like compiled Rust. Contexts
/// that need a total order, sorting for example, go through `compare_values`
/// and keep rejecting NaN.
fn partial_compare(l: &Value, r: &Value) -> Result<Option<Ordering>> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::IntW(..), Value::Int(_) | Value::IntW(..)) | (Value::Int(_), Value::IntW(..)) => {
            let (a, _) = l.int_parts().unwrap();
            let (b, _) = r.int_parts().unwrap();
            Some(a.cmp(&b))
        }
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::F32(a), Value::F32(b)) => a.partial_cmp(b),
        (Value::F32(a), Value::Float(b)) => a.partial_cmp(&(*b as f32)),
        (Value::Float(a), Value::F32(b)) => (*a as f32).partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Str(a), Value::Str(b)) => Some(a.as_str().cmp(b.as_str())),
        (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
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
        (UnKind::Neg, Value::Int(i)) => Value::Int(
            i.checked_neg()
                .ok_or_else(|| anyhow!("attempt to negate with overflow"))?,
        ),
        (UnKind::Neg, Value::IntW(v, w)) => Value::int_of_width(int_neg(*w, w.decode(*v))?, *w),
        (UnKind::Neg, Value::Float(f)) => Value::Float(-*f),
        (UnKind::Neg, Value::F32(f)) => Value::F32(-*f),
        (UnKind::Not, Value::Bool(b)) => Value::Bool(!*b),
        (UnKind::Not, Value::Int(i)) => Value::Int(!*i),
        (UnKind::Not, Value::IntW(v, w)) => Value::int_of_width(int_not(*w, w.decode(*v)), *w),
        (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
    })
}

// -- patterns --------------------------------------------------------------

/// True when a serde_json `Value` variant name matches the native value that a
/// parsed json holds. A json string is a `Str`, a number an `Int` or `Float`, an
/// array a `Vec`, an object a `Map`. `Null` is handled separately as a unit
/// variant because a json null is `Option::None` here.
fn json_variant_kind_matches(name: Option<&str>, val: &Value) -> bool {
    matches!(
        (name, val),
        (Some("String"), Value::Str(_))
            | (Some("Number"), Value::Int(_) | Value::Float(_))
            | (Some("Bool"), Value::Bool(_))
            | (Some("Array"), Value::Vec(_))
            | (Some("Object"), Value::Map(_))
    )
}

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
            other => {
                // A serde_json Value variant pattern, `Value::String(s)` and
                // friends, matched against the native value a parsed json holds.
                // The single field binds to the value itself, the same shape as
                // the pre-unwrapped Some rule below.
                if json_variant_kind_matches(name.as_deref(), other) {
                    bind_seq(elems, from_ref(other), define)
                } else {
                    name.as_deref() == Some("Some") && bind_seq(elems, from_ref(other), define)
                }
            }
        },
        PPat::Path { name } => match val {
            Value::Enum {
                enum_name, variant, ..
            } => {
                name.as_deref() == Some(&**variant)
                    // A json null is Option::None here, so `Value::Null` matches it.
                    || (name.as_deref() == Some("Null")
                        && &**enum_name == "Option"
                        && &**variant == "None")
            }
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
        (PLit::Int(a), Value::IntW(..)) => {
            let (b, _) = value.int_parts()?;
            Some(i128::from(*a).cmp(&b))
        }
        (PLit::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (PLit::Float(a), Value::F32(b)) => (*a as f32).partial_cmp(b),
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
        (PLit::Int(left), Value::IntW(..)) => {
            value.int_parts().map(|(v, _)| v) == Some(i128::from(*left))
        }
        (PLit::Float(left), Value::Float(right)) => left == right,
        (PLit::Float(left), Value::F32(right)) => *left as f32 == *right,
        (PLit::Bool(left), Value::Bool(right)) => left == right,
        (PLit::Str(left), Value::Str(right)) => left == right.as_str(),
        (PLit::Char(left), Value::Char(right)) => left == right,
        _ => false,
    }
}
