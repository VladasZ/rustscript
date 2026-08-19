//! Pattern tests and bindings for the VM's `TestBind` op.

use num_traits::AsPrimitive;
use std::cmp::Ordering;
use std::slice::from_ref;
use std::sync::Arc;

use super::bytecode::{PLit, PPat};
use super::resolver::bare;
use super::value::{List, StructData, Value, ValueRef};

/// Whether a `serde_json::Value` variant pattern like `Value::String(s)`
/// matches the shape of the value, since decoded json is held as plain values.
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

pub(super) fn try_bind(pat: &PPat, val: &Value, define: &mut dyn FnMut(&str, Value)) -> bool {
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
            Value::Tuple(items) => bind_seq(elems, &items.lock(), define),
            Value::Unit if elems.is_empty() => true,
            _ => false,
        },
        PPat::TupleStruct { name, elems } => match val {
            Value::Enum { variant, data, .. } => {
                let payload = data.lock().clone();
                name.as_deref() == Some(&**variant) && bind_seq(elems, &payload, define)
            }
            Value::Struct(st) => {
                let vals: Vec<Value> = st.values.lock().clone();
                bind_seq(elems, &vals, define)
            }
            // A pre-unwrapped Option holds its payload as a plain value, so
            // `Some(x)` still matches one. A `Value::Unit` payload never
            // does, that shape is a real unit value, not an Option.
            Value::Unit => false,
            other => {
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
            Value::Vec(items) => bind_seq(elems, &items.lock(), define),
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
        (PLit::Float(a), Value::F32(b)) => AsPrimitive::<f32>::as_(*a).partial_cmp(b),
        (PLit::Char(a), Value::Char(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Whether a `lo..hi` pattern contains the scrutinee, given a comparator
/// against each bound.
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

/// The `..` slot of a sequence pattern: what a pattern must consume before
/// it, the name a `rest @ ..` binds, and what it must consume after it.
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

fn bind_seq(pats: &[PPat], vals: &[Value], define: &mut dyn FnMut(&str, Value)) -> bool {
    if let Some((head, rest_name, tail)) = split_rest(pats) {
        if vals.len() < head.len() + tail.len() {
            return false;
        }
        let tail_vals = &vals[vals.len() - tail.len()..];
        for (p, v) in head.iter().zip(vals).chain(tail.iter().zip(tail_vals)) {
            if !try_bind(p, v, define) {
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
            .all(|(p, v)| try_bind(p, v, define))
}

fn plit_eq(l: &PLit, val: &Value) -> bool {
    match (l, val) {
        (PLit::Int(a), Value::Int(b)) => a == b,
        (PLit::Int(a), Value::IntW(..)) => val.int_parts().map(|(v, _)| v) == Some(i128::from(*a)),
        (PLit::Float(a), Value::Float(b)) => a == b,
        (PLit::Float(a), Value::F32(b)) => AsPrimitive::<f32>::as_(*a) == *b,
        (PLit::Bool(a), Value::Bool(b)) => a == b,
        (PLit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
        (PLit::Char(a), Value::Char(b)) => a == b,
        _ => false,
    }
}

/// Where a value being bound by reference lives, so the binding can anchor
/// to that storage. A slotless value binds as a plain borrow wrapper when
/// it is a composite, or as a copy when it is a scalar.
enum BindSlot {
    None,
    Elem(List, usize),
    Field(Arc<StructData>, usize),
}

/// Define bindings for a pattern that already matched a `&mut` scrutinee.
/// Every binding anchors to the matched value's own storage where one
/// exists, so `*x += 1` and `v.push(..)` through the binding land in the
/// borrowed place. Runs after `try_bind` said the pattern matches, and must
/// walk the same shapes.
fn bind_refs(pat: &PPat, val: &Value, slot: BindSlot, define: &mut dyn FnMut(&str, Value)) {
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
                bind_refs(s, val, slot, define);
            }
        }
        PPat::Tuple(elems) => {
            if let Value::Tuple(items) = val {
                bind_refs_seq(elems, items, define);
            }
        }
        PPat::TupleStruct { elems, .. } => match val {
            Value::Enum { data, .. } => bind_refs_seq(elems, data, define),
            Value::Struct(st) => {
                let vals: Vec<Value> = st.values.lock().clone();
                for (i, (p, v)) in elems.iter().zip(vals.iter()).enumerate() {
                    bind_refs(p, v, BindSlot::Field(st.clone(), i), define);
                }
            }
            // The pre-unwrapped Some shapes bind the value itself.
            other => {
                if let Some(p) = elems.first() {
                    bind_refs(p, other, BindSlot::None, define);
                }
            }
        },
        PPat::Struct { fields, .. } => {
            if let Value::Struct(st) = val {
                let vals: Vec<Value> = st.values.lock().clone();
                for (fname, p) in fields {
                    if let Some(i) = st.shape.slot(fname) {
                        bind_refs(p, &vals[i], BindSlot::Field(st.clone(), i), define);
                    }
                }
            }
        }
        PPat::Or(alts) => {
            // The first alternative that matches is the one whose bindings
            // are live, the same choice `try_bind` made.
            for alt in alts {
                if try_bind(alt, val, &mut |_, _| {}) {
                    bind_refs(alt, val, slot, define);
                    return;
                }
            }
        }
        PPat::Slice(elems) => {
            if let Value::Vec(items) = val {
                bind_refs_seq(elems, items, define);
            }
        }
        PPat::Wild
        | PPat::Rest
        | PPat::Lit(_)
        | PPat::Path { .. }
        | PPat::Range { .. }
        | PPat::Unsupported => {}
    }
}

/// The element half of `bind_refs`: anchor each pattern to its element slot,
/// with the same split around a `..` that `bind_seq` uses. A named rest
/// binds a copy of the middle elements, not a view, so writing through the
/// rest binding does not reach the scrutinee. Element bindings still do.
fn bind_refs_seq(pats: &[PPat], list: &List, define: &mut dyn FnMut(&str, Value)) {
    let vals: Vec<Value> = list.lock().clone();
    if let Some((head, rest_name, tail)) = split_rest(pats) {
        if vals.len() < head.len() + tail.len() {
            return;
        }
        for (i, p) in head.iter().enumerate() {
            bind_refs(p, &vals[i], BindSlot::Elem(list.clone(), i), define);
        }
        let base = vals.len() - tail.len();
        for (j, p) in tail.iter().enumerate() {
            bind_refs(
                p,
                &vals[base + j],
                BindSlot::Elem(list.clone(), base + j),
                define,
            );
        }
        if let Some(name) = rest_name {
            define(name, Value::vec(vals[head.len()..base].to_vec()));
        }
        return;
    }
    for (i, (p, v)) in pats.iter().zip(vals.iter()).enumerate() {
        bind_refs(p, v, BindSlot::Elem(list.clone(), i), define);
    }
}

/// Entry for the VM: bindings for a matched `&mut` scrutinee.
pub(super) fn bind_pattern_refs(pat: &PPat, val: &Value, define: &mut dyn FnMut(&str, Value)) {
    bind_refs(pat, val, BindSlot::None, define);
}
