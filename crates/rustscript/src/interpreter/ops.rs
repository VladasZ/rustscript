//! Binary and unary operator evaluation plus pattern binding for the
//! register machine. Split from `vm.rs`.

//! The register machine. Executes a compiled `Chunk` against one contiguous
//! register stack. Calls to user functions and closures push a frame record
//! and continue in the same instruction loop, so a script-level call costs no
//! native recursion, no allocation, and no register file copy beyond its
//! arguments. Anything else, methods and std or crate bridges, is delegated to
//! the existing dispatch on `Interp` with already evaluated values.

use std::cmp::Ordering;

use anyhow::{Result, anyhow, bail};
use syn::{Lit, Pat};

use super::bytecode::{BinKind, UnKind};
use super::value::Value;


// -- operators -------------------------------------------------------------

pub(super) fn apply_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    Ok(match op {
        Add | Sub | Mul | Div | Rem => return arith(op, l, r),
        Eq => Value::Bool(l.eq_value(r)),
        Ne => Value::Bool(!l.eq_value(r)),
        Lt => Value::Bool(compare(l, r)? == Ordering::Less),
        Le => Value::Bool(compare(l, r)? != Ordering::Greater),
        Gt => Value::Bool(compare(l, r)? == Ordering::Greater),
        Ge => Value::Bool(compare(l, r)? != Ordering::Less),
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
            Add => Value::Int(a.wrapping_add(imm)),
            Sub => Value::Int(a.wrapping_sub(imm)),
            Mul => Value::Int(a.wrapping_mul(imm)),
            Div => {
                if imm == 0 {
                    bail!("divide by zero");
                }
                Value::Int(a.wrapping_div(imm))
            }
            Rem => {
                if imm == 0 {
                    bail!("remainder by zero");
                }
                Value::Int(a.wrapping_rem(imm))
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
        Lt => compare(l, r)? == Ordering::Less,
        Le => compare(l, r)? != Ordering::Greater,
        Gt => compare(l, r)? == Ordering::Greater,
        Ge => compare(l, r)? != Ordering::Less,
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
            Ok(Value::Int(match op {
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

fn compare(l: &Value, r: &Value) -> Result<Ordering> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Int(a), Value::Float(b)) => {
            (*a as f64).partial_cmp(b).ok_or_else(|| anyhow!("cannot order NaN"))?
        }
        (Value::Float(a), Value::Int(b)) => {
            a.partial_cmp(&(*b as f64)).ok_or_else(|| anyhow!("cannot order NaN"))?
        }
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
pub(super) fn try_bind(pat: &Pat, val: &Value, define: &mut dyn FnMut(&str, Value)) -> bool {
    match pat {
        Pat::Wild(_) | Pat::Rest(_) => true,
        Pat::Ident(id) => {
            if let Some(sub) = &id.subpat
                && !try_bind(&sub.1, val, define)
            {
                return false;
            }
            define(&id.ident.to_string(), val.clone());
            true
        }
        Pat::Lit(lit) => match lit_value(&lit.lit) {
            Some(expected) => expected.eq_value(val),
            None => false,
        },
        Pat::Paren(p) => try_bind(&p.pat, val, define),
        Pat::Reference(r) => try_bind(&r.pat, val, define),
        Pat::Type(t) => try_bind(&t.pat, val, define),
        Pat::Tuple(t) => match val {
            Value::Tuple(items) => bind_seq(t.elems.iter(), &items.borrow(), define),
            _ => false,
        },
        Pat::TupleStruct(ts) => {
            let name = ts.path.segments.last().map(|s| s.ident.to_string());
            match val {
                Value::Enum { variant, data, .. } => {
                    name.as_deref() == Some(&**variant)
                        && bind_seq(ts.elems.iter(), data, define)
                }
                Value::Struct { fields, .. } => {
                    let vals: Vec<Value> = fields.borrow().values().cloned().collect();
                    bind_seq(ts.elems.iter(), &vals, define)
                }
                _ => false,
            }
        }
        Pat::Path(p) => {
            let name = p.path.segments.last().map(|s| s.ident.to_string());
            match val {
                Value::Enum { variant, .. } => name.as_deref() == Some(&**variant),
                _ => false,
            }
        }
        Pat::Struct(s) => {
            let name = s.path.segments.last().map(|s| s.ident.to_string());
            let fields = match val {
                Value::Struct { name: n, fields } => {
                    if let Some(pn) = &name
                        && pn.as_str() != &**n
                    {
                        return false;
                    }
                    fields.borrow()
                }
                _ => return false,
            };
            for f in &s.fields {
                let key = match &f.member {
                    syn::Member::Named(n) => n.to_string(),
                    syn::Member::Unnamed(i) => i.index.to_string(),
                };
                match fields.get(key.as_str()) {
                    Some(v) => {
                        if !try_bind(&f.pat, v, define) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        Pat::Or(or) => or.cases.iter().any(|c| try_bind(c, val, define)),
        Pat::Slice(s) => match val {
            Value::Vec(items) => bind_seq(s.elems.iter(), &items.borrow(), define),
            _ => false,
        },
        _ => false,
    }
}

fn bind_seq<'a>(
    pats: impl Iterator<Item = &'a Pat>,
    vals: &[Value],
    define: &mut dyn FnMut(&str, Value),
) -> bool {
    let pats: Vec<&Pat> = pats.collect();
    if pats.iter().any(|p| matches!(p, Pat::Rest(_))) {
        let head_len = pats.iter().take_while(|p| !matches!(p, Pat::Rest(_))).count();
        for (p, v) in pats.iter().take(head_len).zip(vals.iter()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        let tail: Vec<&&Pat> = pats.iter().skip(head_len + 1).collect();
        for (p, v) in tail.iter().zip(vals.iter().rev()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        return true;
    }
    if pats.len() != vals.len() {
        return false;
    }
    pats.iter().zip(vals.iter()).all(|(p, v)| try_bind(p, v, define))
}

pub(super) fn lit_value(lit: &Lit) -> Option<Value> {
    Some(match lit {
        Lit::Int(i) => Value::Int(i.base10_parse::<i64>().ok()?),
        Lit::Float(f) => Value::Float(f.base10_parse::<f64>().ok()?),
        Lit::Bool(b) => Value::Bool(b.value),
        Lit::Str(s) => Value::str(s.value()),
        Lit::Char(c) => Value::Char(c.value()),
        Lit::Byte(b) => Value::Int(b.value() as i64),
        _ => return None,
    })
}
