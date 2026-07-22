//! Operators and pattern binding for the parallel VM, the `PValue` twin of
//! `ops.rs`. Same logic, different value type.

use std::cmp::Ordering;
use std::slice::from_ref;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, UnKind};
use super::bytecode::{PLit, PPat};
use super::pvalue::PValue;

pub(super) fn apply_bin(op: BinKind, l: &PValue, r: &PValue) -> Result<PValue> {
    use BinKind::*;
    Ok(match op {
        Add | Sub | Mul | Div | Rem => return arith(op, l, r),
        Eq => PValue::Bool(l.eq_value(r)),
        Ne => PValue::Bool(!l.eq_value(r)),
        Lt => PValue::Bool(compare_values(l, r)? == Ordering::Less),
        Le => PValue::Bool(compare_values(l, r)? != Ordering::Greater),
        Gt => PValue::Bool(compare_values(l, r)? == Ordering::Greater),
        Ge => PValue::Bool(compare_values(l, r)? != Ordering::Less),
        BitAnd => int_bin(l, r, |a, b| a & b)?,
        BitOr => int_bin(l, r, |a, b| a | b)?,
        BitXor => int_bin(l, r, |a, b| a ^ b)?,
        Shl => int_bin(l, r, |a, b| a << b)?,
        Shr => int_bin(l, r, |a, b| a >> b)?,
    })
}

pub(super) fn apply_bin_imm(op: BinKind, l: &PValue, imm: i64) -> Result<PValue> {
    apply_bin(op, l, &PValue::Int(imm))
}

pub(super) fn cmp_test(op: BinKind, l: &PValue, r: &PValue) -> Result<bool> {
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

pub(super) fn cmp_test_imm(op: BinKind, l: &PValue, imm: i64) -> Result<bool> {
    cmp_test(op, l, &PValue::Int(imm))
}

fn arith(op: BinKind, l: &PValue, r: &PValue) -> Result<PValue> {
    use BinKind::*;
    if let (Add, PValue::Str(a), PValue::Str(b)) = (op, l, r) {
        let mut out = String::with_capacity(a.len() + b.len());
        out.push_str(a);
        out.push_str(b);
        return Ok(PValue::str(out));
    }
    match (l, r) {
        (PValue::Int(a), PValue::Int(b)) => {
            let (a, b) = (*a, *b);
            Ok(PValue::Int(match op {
                Add => a.wrapping_add(b),
                Sub => a.wrapping_sub(b),
                Mul => a.wrapping_mul(b),
                Div => {
                    if b == 0 {
                        bail!("divide by zero");
                    }
                    a.wrapping_div(b)
                }
                Rem => {
                    if b == 0 {
                        bail!("remainder by zero");
                    }
                    a.wrapping_rem(b)
                }
                _ => unreachable!(),
            }))
        }
        (a, b) => {
            let (x, y) = (to_float(a)?, to_float(b)?);
            Ok(PValue::Float(match op {
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

fn int_bin(l: &PValue, r: &PValue, f: impl Fn(i64, i64) -> i64) -> Result<PValue> {
    match (l, r) {
        (PValue::Int(a), PValue::Int(b)) => Ok(PValue::Int(f(*a, *b))),
        (PValue::Bool(a), PValue::Bool(b)) => Ok(PValue::Bool(f(*a as i64, *b as i64) != 0)),
        _ => bail!("bitwise operators need integers"),
    }
}

pub(super) fn compare_values(l: &PValue, r: &PValue) -> Result<Ordering> {
    Ok(match (l, r) {
        (PValue::Int(a), PValue::Int(b)) => a.cmp(b),
        (PValue::Float(a), PValue::Float(b)) => a
            .partial_cmp(b)
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (PValue::Int(a), PValue::Float(b)) => (*a as f64)
            .partial_cmp(b)
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (PValue::Float(a), PValue::Int(b)) => a
            .partial_cmp(&(*b as f64))
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (PValue::Str(a), PValue::Str(b)) => a.as_ref().cmp(b.as_ref()),
        (PValue::Char(a), PValue::Char(b)) => a.cmp(b),
        (PValue::Bool(a), PValue::Bool(b)) => a.cmp(b),
        (a, b) => bail!("cannot compare {} and {}", a.type_name(), b.type_name()),
    })
}

fn to_float(v: &PValue) -> Result<f64> {
    match v {
        PValue::Int(i) => Ok(*i as f64),
        PValue::Float(f) => Ok(*f),
        other => bail!("expected a number, got {}", other.type_name()),
    }
}

pub(super) fn apply_un(op: UnKind, v: &PValue) -> Result<PValue> {
    Ok(match (op, v) {
        (UnKind::Neg, PValue::Int(i)) => PValue::Int(-*i),
        (UnKind::Neg, PValue::Float(f)) => PValue::Float(-*f),
        (UnKind::Not, PValue::Bool(b)) => PValue::Bool(!*b),
        (UnKind::Not, PValue::Int(i)) => PValue::Int(!*i),
        (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
    })
}

pub(super) fn try_bind(pat: &PPat, val: &PValue, define: &mut dyn FnMut(&str, PValue)) -> bool {
    match pat {
        PPat::Wild | PPat::Rest => true,
        PPat::Ident { name, sub } => {
            if let Some(s) = sub
                && !try_bind(s, val, define)
            {
                return false;
            }
            define(name, val.clone());
            true
        }
        PPat::Lit(l) => plit_eq(l, val),
        PPat::Tuple(elems) => match val {
            PValue::Tuple(items) => bind_seq(elems, &items.lock(), define),
            PValue::Unit if elems.is_empty() => true,
            _ => false,
        },
        PPat::TupleStruct { name, elems } => match val {
            PValue::Enum { variant, data, .. } => {
                name.as_deref() == Some(&**variant) && bind_seq(elems, data, define)
            }
            PValue::Struct(st) => {
                let vals: Vec<PValue> = st.values.lock().clone();
                bind_seq(elems, &vals, define)
            }
            // Matches the pre-unwrapped Some rule in ops.rs, see the note there.
            PValue::Unit => false,
            other => name.as_deref() == Some("Some") && bind_seq(elems, from_ref(other), define),
        },
        PPat::Path { name } => match val {
            PValue::Enum { variant, .. } => name.as_deref() == Some(&**variant),
            _ => false,
        },
        PPat::Struct { name, fields } => {
            let PValue::Struct(st) = val else {
                return false;
            };
            if let Some(pn) = name
                && pn.as_str() != super::resolver::bare(st.name())
            {
                return false;
            }
            for (key, fp) in fields {
                match st.get(key) {
                    Some(v) => {
                        if !try_bind(fp, &v, define) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        PPat::Or(cases) => cases.iter().any(|c| try_bind(c, val, define)),
        PPat::Slice(elems) => match val {
            PValue::Vec(items) => bind_seq(elems, &items.lock(), define),
            _ => false,
        },
        PPat::Unsupported => false,
    }
}

fn bind_seq(pats: &[PPat], vals: &[PValue], define: &mut dyn FnMut(&str, PValue)) -> bool {
    if pats.iter().any(|p| matches!(p, PPat::Rest)) {
        let head = pats.iter().take_while(|p| !matches!(p, PPat::Rest)).count();
        for (p, v) in pats.iter().take(head).zip(vals.iter()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        for (p, v) in pats.iter().skip(head + 1).zip(vals.iter().rev()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        return true;
    }
    pats.len() == vals.len()
        && pats
            .iter()
            .zip(vals.iter())
            .all(|(p, v)| try_bind(p, v, define))
}

fn plit_eq(l: &PLit, val: &PValue) -> bool {
    match (l, val) {
        (PLit::Int(a), PValue::Int(b)) => a == b,
        (PLit::Float(a), PValue::Float(b)) => a == b,
        (PLit::Bool(a), PValue::Bool(b)) => a == b,
        (PLit::Str(a), PValue::Str(b)) => a.as_str() == b.as_ref(),
        (PLit::Char(a), PValue::Char(b)) => a == b,
        _ => false,
    }
}

/// An integer operand, for range bounds and sequence indexes.
pub(super) fn int_of(v: &PValue) -> Result<i64> {
    match v {
        PValue::Int(i) => Ok(*i),
        _ => bail!("range bound must be an integer"),
    }
}

// -- indexing and `?` ------------------------------------------------------

pub(super) fn index(recv: &PValue, key: &PValue) -> Result<PValue> {
    if let PValue::Range {
        start,
        end,
        inclusive,
    } = key
    {
        return slice_value(recv, *start, *end, *inclusive);
    }
    match recv {
        PValue::Vec(items) => {
            let i = int_of(key)? as usize;
            items
                .lock()
                .get(i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("index {i} out of bounds"))
        }
        PValue::Map(m) => {
            let k = key
                .as_key()
                .ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            m.lock()
                .get(&k)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("key not found"))
        }
        PValue::Str(s) => {
            let i = int_of(key)? as usize;
            s.chars()
                .nth(i)
                .map(PValue::Char)
                .ok_or_else(|| anyhow::anyhow!("index out of bounds"))
        }
        // `caps[1]` and `caps["name"]` on a capture set.
        PValue::Native(h) => super::pregex::capture_index(h, key),
        _ => bail!("cannot index {}", recv.type_name()),
    }
}

fn slice_value(base: &PValue, start: i64, end: i64, inclusive: bool) -> Result<PValue> {
    let bounds = |len: usize| -> Result<(usize, usize)> {
        if start < 0 {
            bail!("negative slice start {start}");
        }
        let end = if end == i64::MAX {
            len as i64
        } else if inclusive {
            end + 1
        } else {
            end
        };
        if end < start || end as usize > len {
            bail!("slice {start}..{end} out of bounds (len {len})");
        }
        Ok((start as usize, end as usize))
    };
    match base {
        PValue::Vec(items) => {
            let items = items.lock();
            let (a, b) = bounds(items.len())?;
            Ok(PValue::vec(items[a..b].to_vec()))
        }
        PValue::Str(s) => {
            let (a, b) = bounds(s.len())?;
            match s.get(a..b) {
                Some(sub) => Ok(PValue::str(sub.to_string())),
                None => bail!("slice {a}..{b} is not on a char boundary"),
            }
        }
        other => bail!("cannot slice {}", other.type_name()),
    }
}

pub(super) fn set_index(recv: &PValue, key: &PValue, v: PValue) -> Result<()> {
    match recv {
        PValue::Vec(items) => {
            let i = int_of(key)? as usize;
            let mut items = items.lock();
            if i >= items.len() {
                bail!("index {i} out of bounds");
            }
            items[i] = v;
        }
        PValue::Map(m) => {
            let k = key
                .as_key()
                .ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            m.lock().insert(k, v);
        }
        _ => bail!("cannot index {}", recv.type_name()),
    }
    Ok(())
}

pub(super) fn eval_try(v: PValue) -> Result<std::result::Result<PValue, PValue>> {
    match v {
        PValue::Enum {
            enum_name,
            variant,
            data,
        } => match (&*enum_name, &*variant) {
            ("Result", "Ok") | ("Option", "Some") => {
                Ok(Ok(data.first().cloned().unwrap_or(PValue::Unit)))
            }
            ("Result", "Err") => Ok(Err(PValue::err(
                data.first().cloned().unwrap_or(PValue::Unit),
            ))),
            ("Option", "None") => Ok(Err(PValue::none())),
            _ => bail!("`?` on a non Result/Option value"),
        },
        _ => bail!("`?` on a {}", v.type_name()),
    }
}
