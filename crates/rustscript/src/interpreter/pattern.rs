use num_traits::AsPrimitive;
use std::cmp::Ordering;
use std::slice::from_ref;
use std::sync::Arc;

use super::bytecode::{PLit, PPat};
use super::enum_def::{EnumKind, NONE};
use super::ops::values_equal;
use super::resolver::bare;
use super::value::{List, StructData, Value, ValueRef};

/// Decoded json is held as plain values, so `Value::String(s)` matches by shape.
fn json_variant_kind_matches(name: Option<&str>, val: &Value) -> bool {
    matches!(
        (name, val),
        (Some("String"), Value::Str(_))
            | (Some("Number"), Value::Int(_) | Value::Float(_))
            | (Some("Bool"), Value::Bool(_))
            | (Some("Array"), Value::Vec(_))
            | (Some("Object"), Value::Map(..))
    )
}

/// `consts` holds the value of every constant the pattern names, in `PatInfo::consts` order.
pub(super) fn try_bind(
    pat: &PPat,
    val: &Value,
    consts: &[Value],
    define: &mut dyn FnMut(&str, Value),
) -> bool {
    match pat {
        PPat::Wild | PPat::Rest => true,
        PPat::Ident { name, sub } => {
            if let Some(s) = sub
                && !try_bind(s, val, consts, define)
            {
                return false;
            }
            define(name, val.clone());
            true
        }
        PPat::Lit(l) => plit_eq(l, val),
        PPat::Const(idx) => consts
            .get(usize::from(*idx))
            .is_some_and(|c| values_equal(c, val)),
        PPat::Tuple(elems) => match val {
            Value::Tuple(items) => bind_seq(elems, &items.lock(), consts, define),
            Value::Unit if elems.is_empty() => true,
            _ => false,
        },
        PPat::TupleStruct { tag, elems } => match val {
            Value::Enum { def, variant, data } => {
                if !tag.matches(def, *variant) {
                    return false;
                }
                let payload = data.lock().clone();
                bind_seq(elems, &payload, consts, define)
            }
            Value::Struct(st) => {
                let vals: Vec<Value> = st.values.lock().clone();
                bind_seq(elems, &vals, consts, define)
            }
            // `Some(x)` still matches a pre unwrapped payload, a `Value::Unit` never does, that
            // is a real unit value
            Value::Unit => false,
            other => {
                if json_variant_kind_matches(tag.name.as_deref(), other) {
                    bind_seq(elems, from_ref(other), consts, define)
                } else {
                    tag.is_named("Some") && bind_seq(elems, from_ref(other), consts, define)
                }
            }
        },
        PPat::Path { tag } => match val {
            Value::Enum { def, variant, .. } => {
                tag.matches(def, *variant)
                    // a json null is `Option::None`
                    || (tag.is_named("Null") && def.kind == EnumKind::Option && *variant == NONE)
            }
            _ => false,
        },
        PPat::Struct { name, fields } => {
            let Value::Struct(st) = val else {
                return false;
            };
            if let Some(pn) = name
                && pn.as_str() != bare(st.name())
            {
                return false;
            }
            for (key, fp) in fields {
                match st.get(key) {
                    Some(v) => {
                        if !try_bind(fp, &v, consts, define) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        PPat::Or(cases) => cases.iter().any(|c| try_bind(c, val, consts, define)),
        PPat::Slice(elems) => match val {
            Value::Vec(items) => bind_seq(elems, &items.lock(), consts, define),
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

/// `None` for a type mismatch, which makes the range not match.
fn endpoint_cmp(literal: &PLit, value: &Value) -> Option<Ordering> {
    match (literal, value) {
        (PLit::Int(a), Value::Int(_) | Value::IntW(..) | Value::Big(..)) => {
            let (b, _) = value.int_parts()?;
            Some(a.cmp(&b))
        }
        (PLit::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (PLit::Float(a), Value::F32(b)) => AsPrimitive::<f32>::as_(*a).partial_cmp(b),
        (PLit::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn range_matches<L>(
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

/// The `..` slot of a sequence pattern.
fn split_rest(pats: &[PPat]) -> Option<(&[PPat], Option<&str>, &[PPat])> {
    let pos = pats.iter().position(is_rest)?;
    let name = match &pats[pos] {
        PPat::Ident { name, .. } => Some(name.as_str()),
        _ => None,
    };
    Some((&pats[..pos], name, &pats[pos + 1..]))
}

fn is_rest(pat: &PPat) -> bool {
    match pat {
        PPat::Rest => true,
        PPat::Ident { sub: Some(sub), .. } => matches!(**sub, PPat::Rest),
        _ => false,
    }
}

fn bind_seq(
    pats: &[PPat],
    vals: &[Value],
    consts: &[Value],
    define: &mut dyn FnMut(&str, Value),
) -> bool {
    if let Some((head, rest_name, tail)) = split_rest(pats) {
        if vals.len() < head.len() + tail.len() {
            return false;
        }
        let tail_vals = &vals[vals.len() - tail.len()..];
        for (p, v) in head.iter().zip(vals).chain(tail.iter().zip(tail_vals)) {
            if !try_bind(p, v, consts, define) {
                return false;
            }
        }
        if let Some(name) = rest_name {
            let middle = vals[head.len()..vals.len() - tail.len()].to_vec();
            define(name, Value::vec(middle));
        }
        return true;
    }
    pats.len() == vals.len()
        && pats
            .iter()
            .zip(vals.iter())
            .all(|(p, v)| try_bind(p, v, consts, define))
}

fn plit_eq(l: &PLit, val: &Value) -> bool {
    match (l, val) {
        (PLit::Int(a), Value::Int(_) | Value::IntW(..) | Value::Big(..)) => {
            val.int_parts().map(|(v, _)| v) == Some(*a)
        }
        (PLit::Float(a), Value::Float(b)) => a == b,
        (PLit::Float(a), Value::F32(b)) => AsPrimitive::<f32>::as_(*a) == *b,
        (PLit::Bool(a), Value::Bool(b)) => a == b,
        (PLit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
        (PLit::Char(a), Value::Char(b)) => a == b,
        _ => false,
    }
}

/// A slotless value binds as a borrow wrapper when composite, as a copy when scalar.
enum BindSlot {
    None,
    Elem(List, usize),
    Field(Arc<StructData>, usize),
}

/// Every binding anchors to the matched storage, so `*x += 1` through it lands in the place. Runs
/// after `try_bind` matched and must walk the same shapes.
fn bind_refs(
    pat: &PPat,
    val: &Value,
    slot: BindSlot,
    consts: &[Value],
    define: &mut dyn FnMut(&str, Value),
) {
    match pat {
        PPat::Ident { name, sub } => {
            let bound = match &slot {
                BindSlot::Elem(list, i) => {
                    Value::Ref(Arc::new(ValueRef::vec_element(list.clone(), *i)))
                }
                BindSlot::Field(data, i) => {
                    Value::Ref(Arc::new(ValueRef::struct_field(data.clone(), *i)))
                }
                BindSlot::None => match val {
                    Value::Vec(_)
                    | Value::Map(..)
                    | Value::Tuple(_)
                    | Value::Struct(_)
                    | Value::Enum { .. } => Value::Ref(Arc::new(ValueRef::borrowed(val.clone()))),
                    other => other.clone(),
                },
            };
            define(name, bound);
            if let Some(s) = sub {
                bind_refs(s, val, slot, consts, define);
            }
        }
        PPat::Tuple(elems) => {
            if let Value::Tuple(items) = val {
                bind_refs_seq(elems, items, consts, define);
            }
        }
        PPat::TupleStruct { elems, .. } => match val {
            Value::Enum { data, .. } => bind_refs_seq(elems, data, consts, define),
            Value::Struct(st) => {
                let vals: Vec<Value> = st.values.lock().clone();
                for (i, (p, v)) in elems.iter().zip(vals.iter()).enumerate() {
                    bind_refs(p, v, BindSlot::Field(st.clone(), i), consts, define);
                }
            }
            other => {
                if let Some(p) = elems.first() {
                    bind_refs(p, other, BindSlot::None, consts, define);
                }
            }
        },
        PPat::Struct { fields, .. } => {
            if let Value::Struct(st) = val {
                let vals: Vec<Value> = st.values.lock().clone();
                for (fname, p) in fields {
                    if let Some(i) = st.shape.slot(fname) {
                        bind_refs(p, &vals[i], BindSlot::Field(st.clone(), i), consts, define);
                    }
                }
            }
        }
        PPat::Or(alts) => {
            // the first matching alternative, the same choice `try_bind` made
            for alt in alts {
                if try_bind(alt, val, consts, &mut |_, _| {}) {
                    bind_refs(alt, val, slot, consts, define);
                    return;
                }
            }
        }
        PPat::Slice(elems) => {
            if let Value::Vec(items) = val {
                bind_refs_seq(elems, items, consts, define);
            }
        }
        PPat::Wild
        | PPat::Rest
        | PPat::Lit(_)
        | PPat::Const(_)
        | PPat::Path { .. }
        | PPat::Range { .. }
        | PPat::Unsupported => {}
    }
}

/// The element half of `bind_refs`. A named rest binds a copy, so writing through it doesn't
/// reach the scrutinee. Element bindings still do.
fn bind_refs_seq(
    pats: &[PPat],
    list: &List,
    consts: &[Value],
    define: &mut dyn FnMut(&str, Value),
) {
    let vals: Vec<Value> = list.lock().clone();
    if let Some((head, rest_name, tail)) = split_rest(pats) {
        if vals.len() < head.len() + tail.len() {
            return;
        }
        for (i, p) in head.iter().enumerate() {
            bind_refs(p, &vals[i], BindSlot::Elem(list.clone(), i), consts, define);
        }
        let base = vals.len() - tail.len();
        for (j, p) in tail.iter().enumerate() {
            bind_refs(
                p,
                &vals[base + j],
                BindSlot::Elem(list.clone(), base + j),
                consts,
                define,
            );
        }
        if let Some(name) = rest_name {
            define(name, Value::vec(vals[head.len()..base].to_vec()));
        }
        return;
    }
    for (i, (p, v)) in pats.iter().zip(vals.iter()).enumerate() {
        bind_refs(p, v, BindSlot::Elem(list.clone(), i), consts, define);
    }
}

pub(super) fn bind_pattern_refs(
    pat: &PPat,
    val: &Value,
    consts: &[Value],
    define: &mut dyn FnMut(&str, Value),
) {
    bind_refs(pat, val, BindSlot::None, consts, define);
}
