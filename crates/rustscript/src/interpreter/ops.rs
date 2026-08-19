//! Operators for the VM. Pattern tests and bindings live in `pattern.rs`.

use num_traits::AsPrimitive;
use std::cmp::Ordering;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, UnKind};
use super::numeric::{
    IntWidth, float_arith, i64_arith, int_arith, int_bit, int_neg, int_not, int_shift, u64_arith,
    unify,
};
use super::shared::{duration_arith, usize_i64};
use super::std_bridge::{duration_from_value, make_duration};
use super::value::Value;

pub(super) fn apply_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::{
        Add, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Rem, Shl, Shr, Sub,
    };
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

pub(super) fn apply_bin_imm(op: BinKind, l: &Value, imm: i64) -> Result<Value> {
    apply_bin(op, l, &Value::Int(imm))
}

pub(super) fn cmp_test(op: BinKind, l: &Value, r: &Value) -> Result<bool> {
    use BinKind::{Eq, Ge, Gt, Le, Lt, Ne};
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
    cmp_test(op, l, &Value::Int(imm))
}

fn arith(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    // Same-type numbers first, they dominate hot loops. The remaining
    // patterns are disjoint from these two, so the order change is safe.
    if let (Value::Int(a), Value::Int(b)) = (l, r) {
        return Ok(Value::Int(i64_arith(op, *a, *b)?));
    }
    // A 64-bit unsigned pair, or one with a bare literal beside it, computes
    // natively instead of through the i128 pipeline. Counting and checksum
    // loops on `u64` and `usize` live here.
    if let Value::IntW(a, wa @ (IntWidth::U64 | IntWidth::USize)) = l {
        let rhs = match r {
            Value::IntW(b, wb) if wa == wb => Some(b.cast_unsigned()),
            Value::Int(b) if *b >= 0 => Some(b.cast_unsigned()),
            _ => None,
        };
        if let Some(y) = rhs {
            let x = a.cast_unsigned();
            let out = u64_arith(op, x, y)?;
            return Ok(Value::IntW(out.cast_signed(), *wa));
        }
    }
    if let (Value::Float(a), Value::Float(b)) = (l, r) {
        return Ok(Value::Float(float_arith(op, *a, *b)));
    }
    if let (BinKind::Add, Value::Str(a), Value::Str(b)) = (op, l, r) {
        let mut out = String::with_capacity(a.len() + b.len());
        out.push_str(a);
        out.push_str(b);
        return Ok(Value::str(out));
    }
    // Only a struct can be a Duration, so the discriminant check keeps the
    // probe off every numeric op.
    if matches!(l, Value::Struct(_))
        && let (Some(a), Some(b)) = (duration_from_value(l), duration_from_value(r))
    {
        return Ok(make_duration(duration_arith(op, a, b)?));
    }
    if let Some(width) = big_operands(l, r) {
        let (a, b) = (big_bits(l), big_bits(r));
        return Ok(Value::Big(
            super::numeric::big_arith(op, width, a, b)?,
            width,
        ));
    }
    if let (Some((a, wa)), Some((b, wb))) = (l.int_parts(), r.int_parts()) {
        let width = unify(wa, wb)?;
        return Ok(Value::int_of_width(int_arith(op, width, a, b)?, width));
    }
    match float_pair(l, r)? {
        FloatPair::F64(x, y) => Ok(Value::Float(float_arith(op, x, y))),
        FloatPair::F32(x, y) => Ok(Value::F32(float_arith(op, x, y))),
    }
}

/// An untagged f64 next to an f32 is a bare
/// literal that is f32 in the source types.
enum FloatPair {
    F64(f64, f64),
    F32(f32, f32),
}

fn float_pair(l: &Value, r: &Value) -> Result<FloatPair> {
    Ok(match (l, r) {
        (Value::F32(a), Value::F32(b)) => FloatPair::F32(*a, *b),
        (Value::F32(a), Value::Float(b)) => FloatPair::F32(*a, AsPrimitive::<f32>::as_(*b)),
        (Value::Float(a), Value::F32(b)) => FloatPair::F32(AsPrimitive::<f32>::as_(*a), *b),
        (a, b) => FloatPair::F64(to_float(a)?, to_float(b)?),
    })
}

fn bit_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    if let (Value::Int(a), Value::Int(b)) = (l, r) {
        let f = bit_i64(op);
        return Ok(Value::Int(f(*a, *b)));
    }
    if let (Value::Bool(a), Value::Bool(b)) = (l, r) {
        let f = bit_i64(op);
        return Ok(Value::Bool(f(i64::from(*a), i64::from(*b)) != 0));
    }
    if let Some(width) = big_operands(l, r) {
        let (a, b) = (big_bits(l), big_bits(r));
        return Ok(Value::Big(
            super::numeric::big_arith(op, width, a, b)?,
            width,
        ));
    }
    if let (Some((a, wa)), Some((b, wb))) = (l.int_parts(), r.int_parts()) {
        let width = unify(wa, wb)?;
        return Ok(Value::int_of_width(int_bit(op, a, b)?, width));
    }
    bail!("bitwise operators need integers")
}

fn bit_i64(op: BinKind) -> fn(i64, i64) -> i64 {
    match op {
        BinKind::BitAnd => |a, b| a & b,
        BinKind::BitOr => |a, b| a | b,
        _ => |a, b| a ^ b,
    }
}

/// `<<` and `>>`, the amount only supplies the count and the result keeps
/// the shifted side's width.
fn shift_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    if let Value::Big(a, w) = l {
        let Some((amount, _)) = r.int_parts() else {
            bail!("shift operators need integers");
        };
        return Ok(Value::Big(int_shift(op, *w, *a, amount)?, *w));
    }
    let (Some((a, wa)), Some((b, _))) = (l.int_parts(), r.int_parts()) else {
        bail!("shift operators need integers");
    };
    Ok(Value::int_of_width(int_shift(op, wa, a, b)?, wa))
}

pub(super) fn compare_values(l: &Value, r: &Value) -> Result<Ordering> {
    partial_compare(l, r)?.ok_or_else(|| anyhow!("cannot order NaN"))
}

/// `PartialOrd` semantics: a NaN operand makes every ordered
/// comparison false, exactly like compiled Rust. Contexts that need a total
/// order, sorting for example, go through `compare_values` and reject NaN.
fn partial_compare(l: &Value, r: &Value) -> Result<Option<Ordering>> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Big(a, wa), Value::Big(b, _)) => Some(if *wa == super::numeric::IntWidth::U128 {
            a.cast_unsigned().cmp(&b.cast_unsigned())
        } else {
            a.cmp(b)
        }),
        (Value::Big(..), Value::Int(_)) | (Value::Int(_), Value::Big(..)) => {
            match (l.int_parts(), r.int_parts()) {
                (Some((a, _)), Some((b, _))) => Some(a.cmp(&b)),
                // The big side is a u128 past the i128 range, larger than
                // any i64 the other side can hold.
                (None, _) => Some(Ordering::Greater),
                (_, None) => Some(Ordering::Less),
            }
        }
        (Value::IntW(..), Value::Int(_) | Value::IntW(..)) | (Value::Int(_), Value::IntW(..)) => {
            let (a, _) = l.int_parts().unwrap();
            let (b, _) = r.int_parts().unwrap();
            Some(a.cmp(&b))
        }
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::F32(a), Value::F32(b)) => a.partial_cmp(b),
        (Value::F32(a), Value::Float(b)) => a.partial_cmp(&AsPrimitive::<f32>::as_(*b)),
        (Value::Float(a), Value::F32(b)) => AsPrimitive::<f32>::as_(*a).partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => AsPrimitive::<f64>::as_(*a).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&AsPrimitive::<f64>::as_(*b)),
        (Value::Str(a), Value::Str(b)) => Some(a.as_ref().cmp(b.as_ref())),
        (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        // Sequences order lexicographically, the first differing element
        // deciding and a prefix ordering before what extends it. Tuples order
        // the same way, field by field.
        (Value::Vec(a), Value::Vec(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
            // Two separate statements on purpose. In one statement both lock
            // guards live to the end of it, so ordering a value against its
            // own clone locks the same mutex twice and deadlocks.
            let a = a.lock().clone();
            let b = b.lock().clone();
            let mut order = None;
            for (left, right) in a.iter().zip(b.iter()) {
                match partial_compare(left, right)? {
                    Some(Ordering::Equal) => {}
                    other => {
                        order = Some(other);
                        break;
                    }
                }
            }
            match order {
                Some(decided) => decided,
                None => Some(a.len().cmp(&b.len())),
            }
        }
        // `Option` and `Result` derive their order from the variant order in
        // the declaration, so `None` sorts before any `Some` and `Ok` before
        // any `Err`, and two of the same variant compare by payload.
        (
            Value::Enum {
                enum_name: left_enum,
                variant: left_variant,
                data: left_data,
            },
            Value::Enum {
                enum_name: right_enum,
                variant: right_variant,
                data: right_data,
            },
        ) if left_enum == right_enum => {
            let rank = |variant: &str| match variant {
                "None" | "Ok" => 0,
                _ => 1,
            };
            match rank(left_variant).cmp(&rank(right_variant)) {
                Ordering::Equal => {
                    // Snapshots, not held guards: comparing a value with its
                    // own clone sees the same storage on both sides.
                    let left_data = left_data.lock().clone();
                    let right_data = right_data.lock().clone();
                    let mut order = None;
                    for (left, right) in left_data.iter().zip(right_data.iter()) {
                        match partial_compare(left, right)? {
                            Some(Ordering::Equal) => {}
                            other => {
                                order = Some(other);
                                break;
                            }
                        }
                    }
                    match order {
                        Some(decided) => decided,
                        None => Some(left_data.len().cmp(&right_data.len())),
                    }
                }
                decided => Some(decided),
            }
        }
        (a, b) => bail!("cannot compare {} and {}", a.type_name(), b.type_name()),
    })
}

fn to_float(v: &Value) -> Result<f64> {
    match v {
        Value::Int(i) => Ok(AsPrimitive::<f64>::as_(*i)),
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
        (UnKind::Neg, Value::Big(v, w)) => Value::Big(int_neg(*w, *v)?, *w),
        (UnKind::Neg, Value::Float(f)) => Value::Float(-*f),
        (UnKind::Neg, Value::F32(f)) => Value::F32(-*f),
        (UnKind::Not, Value::Bool(b)) => Value::Bool(!*b),
        (UnKind::Not, Value::Int(i)) => Value::Int(!*i),
        (UnKind::Not, Value::IntW(v, w)) => Value::int_of_width(int_not(*w, w.decode(*v)), *w),
        (UnKind::Not, Value::Big(v, w)) => Value::Big(int_not(*w, *v), *w),
        (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
    })
}

/// An integer operand, for range bounds and sequence indexes.
pub(super) fn int_of(v: &Value) -> Result<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::IntW(..) => v
            .untag_int()
            .ok_or_else(|| anyhow!("integer out of the i64 range")),
        Value::Big(..) => match v.int_parts() {
            Some((n, _)) => i64::try_from(n).map_err(|_| anyhow!("integer out of the i64 range")),
            None => bail!("integer out of the i64 range"),
        },
        _ => bail!("range bound must be an integer"),
    }
}

/// The shared 128-bit width of two operands when either is a `Value::Big`.
/// The untagged side is a bare literal adopting the big side's width.
fn big_operands(l: &Value, r: &Value) -> Option<super::numeric::IntWidth> {
    match (l, r) {
        (Value::Big(_, w), Value::Big(..) | Value::Int(_)) | (Value::Int(_), Value::Big(_, w)) => {
            Some(*w)
        }
        _ => None,
    }
}

/// An operand's raw 128-bit image for the big paths: a `Big` as stored, an
/// untagged literal widened.
fn big_bits(v: &Value) -> i128 {
    match v {
        Value::Big(bits, _) => *bits,
        Value::Int(i) => i128::from(*i),
        _ => 0,
    }
}

// -- indexing and `?` ------------------------------------------------------

pub(super) fn index(recv: &Value, key: &Value) -> Result<Value> {
    if let Value::Range {
        start,
        end,
        inclusive,
    } = key
    {
        return slice_value(recv, *start, *end, *inclusive);
    }
    match recv {
        Value::Vec(items) => {
            let i = usize::try_from(int_of(key)?)?;
            let items = items.lock();
            items.get(i).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "index out of bounds: the len is {} but the index is {i}",
                    items.len()
                )
            })
        }
        Value::Map(m, _) => {
            let k = key
                .as_key()
                .ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            m.lock()
                .get(&k)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no entry found for key"))
        }
        Value::Str(s) => {
            let i = usize::try_from(int_of(key)?)?;
            s.chars().nth(i).map(Value::Char).ok_or_else(|| {
                anyhow::anyhow!(
                    "index out of bounds: the len is {} but the index is {i}",
                    s.chars().count()
                )
            })
        }
        // `caps[1]` and `caps["name"]` on a capture set.
        Value::Native(h) => super::regex_bridge::capture_index(h, key),
        _ => bail!("cannot index {}", recv.type_name()),
    }
}

/// A range key resolved against a length. The messages are the exact texts
/// debug Rust panics with, checked in declaration order: inverted range
/// here, out of bounds and a string's char boundary at the use site.
fn range_bounds(len: usize, start: i64, end: i64, inclusive: bool) -> Result<(usize, usize)> {
    if start < 0 {
        bail!("negative slice start {start}");
    }
    let end = if end == i64::MAX {
        usize_i64(len)
    } else if inclusive {
        end + 1
    } else {
        end
    };
    if end < start {
        bail!("slice index starts at {start} but ends at {end}");
    }
    Ok((usize::try_from(start)?, usize::try_from(end)?))
}

/// The debug Rust panic text for a byte range off a char boundary.
fn char_boundary_error(s: &str, a: usize, b: usize) -> anyhow::Error {
    let (side, bad) = if s.is_char_boundary(a) {
        ("end", b)
    } else {
        ("start", a)
    };
    let mut at = bad;
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    let ch = s[at..].chars().next().unwrap_or('\u{FFFD}');
    anyhow!(
        "{side} byte index {bad} is not a char boundary; it is inside {ch:?} (bytes {at}..{} of string)",
        at + ch.len_utf8()
    )
}

fn slice_value(base: &Value, start: i64, end: i64, inclusive: bool) -> Result<Value> {
    match base {
        Value::Vec(items) => {
            let items = items.lock();
            let (a, b) = range_bounds(items.len(), start, end, inclusive)?;
            if b > items.len() {
                bail!(
                    "range end index {b} out of range for slice of length {}",
                    items.len()
                );
            }
            Ok(Value::vec(items[a..b].to_vec()))
        }
        Value::Str(s) => {
            let (a, b) = range_bounds(s.len(), start, end, inclusive)?;
            if b > s.len() {
                bail!(
                    "end byte index {b} is out of bounds for string of length {}",
                    s.len()
                );
            }
            match s.get(a..b) {
                Some(sub) => Ok(Value::str(sub.to_string())),
                None => Err(char_boundary_error(s, a, b)),
            }
        }
        other => bail!("cannot slice {}", other.type_name()),
    }
}

/// The writeback of a mutating method called on a string slice, like
/// `s[2..].make_ascii_uppercase()`. The slice arrived as a copied temporary,
/// so the mutated bytes are spliced back into the base string here.
pub(super) fn splice_str(
    s: &str,
    start: i64,
    end: i64,
    inclusive: bool,
    val: &Value,
) -> Result<String> {
    let Value::Str(new) = val else {
        bail!("cannot write {} back into a string slice", val.type_name());
    };
    let (a, b) = range_bounds(s.len(), start, end, inclusive)?;
    if b > s.len() {
        bail!(
            "end byte index {b} is out of bounds for string of length {}",
            s.len()
        );
    }
    if s.get(a..b).is_none() {
        return Err(char_boundary_error(s, a, b));
    }
    let mut out = s.to_string();
    out.replace_range(a..b, new);
    Ok(out)
}

pub(super) fn set_index(recv: &Value, key: &Value, v: Value) -> Result<()> {
    match recv {
        Value::Vec(items) => {
            let i = usize::try_from(int_of(key)?)?;
            let mut items = items.lock();
            if i >= items.len() {
                bail!(
                    "index out of bounds: the len is {} but the index is {i}",
                    items.len()
                );
            }
            items[i] = v;
        }
        Value::Map(m, _) => {
            let k = key
                .as_key()
                .ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            m.lock().insert(k, v);
        }
        _ => bail!("cannot index {}", recv.type_name()),
    }
    Ok(())
}

pub(super) fn eval_try(v: Value) -> Result<Value, Value> {
    match v {
        Value::Enum {
            enum_name,
            variant,
            data,
        } => match (&*enum_name, &*variant) {
            ("Result", "Ok") | ("Option", "Some") => {
                Ok(data.lock().first().cloned().unwrap_or(Value::Unit))
            }
            ("Result", "Err") => {
                let inner = data.lock().first().cloned().unwrap_or(Value::Unit);
                Err(Value::err(inner))
            }
            ("Option", "None") => Err(Value::none()),
            // Any other value acts as its own Some, matching eval_try in
            // eval.rs, see the comment there.
            _ => Ok(Value::Enum {
                enum_name,
                variant,
                data,
            }),
        },
        other => Ok(other),
    }
}
