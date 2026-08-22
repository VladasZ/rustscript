//! The `{:?}` rendering. The spec reaches every leaf, `{:>4?}` of `vec![1]` is `[   1]`, while
//! `str` and `char` never pad.

use std::fmt::Write as _;
use std::sync::Arc;

use parking_lot::Mutex;

use super::format::{SpecNumber, apply_spec};
use super::native::Native;
use super::value::{CellKind, MapKind, StructData, Value, big_text, format_float_debug};

pub(super) struct DebugOpts<'a> {
    pub pretty: bool,
    /// the spec every leaf is formatted with, empty for a bare `{:?}`
    pub leaf: &'a str,
}

impl DebugOpts<'_> {
    pub fn plain() -> DebugOpts<'static> {
        DebugOpts {
            pretty: false,
            leaf: "",
        }
    }
}

pub(super) fn render(value: &Value, opts: &DebugOpts) -> String {
    let mut out = String::new();
    write_value(value, opts, 0, &mut out);
    out
}

/// A float keeps its `1.0` unless a precision rounds it.
fn leaf_number(text: String, number: SpecNumber, opts: &DebugOpts, out: &mut String) {
    if opts.leaf.is_empty() {
        out.push_str(&text);
        return;
    }
    let spec = format!("{}?", opts.leaf);
    out.push_str(&apply_spec(&spec, &text, &text, Some(number), true));
}

fn float_text(f: f64, opts: &DebugOpts) -> String {
    match precision_of(opts.leaf) {
        Some(precision) => format!("{f:.precision$}"),
        None => format_float_debug(f),
    }
}

fn f32_text(f: f32, opts: &DebugOpts) -> String {
    match precision_of(opts.leaf) {
        Some(precision) => format!("{f:.precision$}"),
        None => format!("{f:?}"),
    }
}

/// `2` in `>8.2`
fn precision_of(leaf: &str) -> Option<usize> {
    let after = leaf.split_once('.')?.1;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn write_value(value: &Value, opts: &DebugOpts, indent: usize, out: &mut String) {
    match value {
        Value::Unit => out.push_str("()"),
        // `bool` and the numbers route `Debug` to a padding `Display`
        Value::Bool(_)
        | Value::Int(_)
        | Value::IntW(..)
        | Value::Big(..)
        | Value::Float(_)
        | Value::F32(_)
        | Value::Char(_)
        | Value::Str(_) => write_leaf(value, opts, out),
        Value::Range {
            start,
            end,
            inclusive,
        } => {
            let sep = if *inclusive { "..=" } else { ".." };
            write!(out, "{start}{sep}{end}").unwrap();
        }
        Value::Vec(items) => {
            let items = items.lock().clone();
            write_seq("[", "]", &items, opts, indent, out);
        }
        Value::Tuple(items) => {
            let items = items.lock().clone();
            if items.len() == 1 && !opts.pretty {
                out.push('(');
                write_value(&items[0], opts, indent, out);
                out.push_str(",)");
            } else {
                write_seq("(", ")", &items, opts, indent, out);
            }
        }
        Value::Map(map, kind) => {
            let entries: Vec<(Value, Value)> = map
                .lock()
                .iter()
                .map(|(k, v)| (k.to_value(), v.clone()))
                .collect();
            write_map(&entries, *kind, opts, indent, out);
        }
        Value::Struct(s) => write_struct(s, opts, indent, out),
        Value::Closure(_) => out.push_str("<closure>"),
        Value::Ref(reference) => match reference.get() {
            Some(value) => write_value(&value, opts, indent, out),
            None => out.push_str("<dangling reference>"),
        },
        Value::Cell(kind, slot) => write_cell(*kind, slot, opts, indent, out),
        Value::Native(n) => match &*n.lock() {
            // `ParseIntError { kind: InvalidDigit }` spreads over lines in the pretty form like
            // any struct
            Native::ParseErr { debug, .. } if opts.pretty && debug.contains(" { ") => {
                let (name, rest) = debug.split_once(" { ").unwrap_or((debug, ""));
                let field = rest.trim_end_matches(" }");
                write!(
                    out,
                    "{name} {{\n{}{field},\n{}}}",
                    pad(indent + 1),
                    pad(indent)
                )
                .unwrap();
            }
            Native::IoErr { debug, .. }
            | Native::JoinErr { debug, .. }
            | Native::ParseErr { debug, .. } => out.push_str(debug),
            other => write!(out, "<{}>", other.type_name()).unwrap(),
        },
        Value::Enum { def, variant, data } => {
            out.push_str(def.variant_name(*variant));
            let data = data.lock().clone();
            if !data.is_empty() {
                write_seq("(", ")", &data, opts, indent, out);
            }
        }
    }
}

fn write_leaf(value: &Value, opts: &DebugOpts, out: &mut String) {
    match value {
        Value::Bool(b) => {
            let text = b.to_string();
            if opts.leaf.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&apply_spec(
                    &format!("{}?", opts.leaf),
                    &text,
                    &text,
                    None,
                    true,
                ));
            }
        }
        Value::Int(i) => leaf_number(i.to_string(), SpecNumber::Int(*i), opts, out),
        Value::IntW(v, w) => leaf_number(
            w.decode(*v).to_string(),
            SpecNumber::Sized {
                value: w.decode(*v),
                bits: w.bits(),
            },
            opts,
            out,
        ),
        Value::Big(v, w) => leaf_number(
            big_text(*v, *w),
            SpecNumber::Big {
                bits: *v,
                signed: w.is_signed(),
            },
            opts,
            out,
        ),
        Value::Float(f) => leaf_number(float_text(*f, opts), SpecNumber::Float(*f), opts, out),
        Value::F32(f) => leaf_number(f32_text(*f, opts), SpecNumber::F32(*f), opts, out),
        Value::Char(c) => write!(out, "{c:?}").unwrap(),
        Value::Str(s) => write!(out, "{:?}", &**s).unwrap(),
        _ => unreachable!("write_leaf handles the scalar leaves only"),
    }
}

fn pad(indent: usize) -> String {
    "    ".repeat(indent)
}

/// An empty sequence is `[]` in both forms.
fn write_seq(
    open: &str,
    close: &str,
    items: &[Value],
    opts: &DebugOpts,
    indent: usize,
    out: &mut String,
) {
    out.push_str(open);
    if items.is_empty() {
        out.push_str(close);
        return;
    }
    if opts.pretty {
        out.push('\n');
        for item in items {
            out.push_str(&pad(indent + 1));
            write_value(item, opts, indent + 1, out);
            out.push_str(",\n");
        }
        out.push_str(&pad(indent));
    } else {
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_value(item, opts, indent, out);
        }
    }
    out.push_str(close);
}

fn write_map(
    entries: &[(Value, Value)],
    kind: MapKind,
    opts: &DebugOpts,
    indent: usize,
    out: &mut String,
) {
    out.push('{');
    if entries.is_empty() {
        out.push('}');
        return;
    }
    if opts.pretty {
        out.push('\n');
        for (k, v) in entries {
            out.push_str(&pad(indent + 1));
            write_value(k, opts, indent + 1, out);
            if kind == MapKind::Map {
                out.push_str(": ");
                write_value(v, opts, indent + 1, out);
            }
            out.push_str(",\n");
        }
        out.push_str(&pad(indent));
    } else {
        for (i, (k, v)) in entries.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_value(k, opts, indent, out);
            if kind == MapKind::Map {
                out.push_str(": ");
                write_value(v, opts, indent, out);
            }
        }
    }
    out.push('}');
}

/// `Name`, `Name(a, b)` or `Name { f: v }`
fn write_struct(s: &StructData, opts: &DebugOpts, indent: usize, out: &mut String) {
    out.push_str(super::resolver::bare(s.name()));
    let values = s.values.lock().clone();
    if values.is_empty() {
        return;
    }
    let tuple_like = s
        .shape
        .fields
        .iter()
        .enumerate()
        .all(|(i, f)| **f == i.to_string());
    if tuple_like {
        write_seq("(", ")", &values, opts, indent, out);
        return;
    }
    if opts.pretty {
        out.push_str(" {\n");
        for (k, v) in s.shape.fields.iter().zip(values.iter()) {
            out.push_str(&pad(indent + 1));
            write!(out, "{k}: ").unwrap();
            write_value(v, opts, indent + 1, out);
            out.push_str(",\n");
        }
        out.push_str(&pad(indent));
        out.push('}');
    } else {
        out.push_str(" { ");
        for (i, (k, v)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{k}: ").unwrap();
            write_value(v, opts, indent, out);
        }
        out.push_str(" }");
    }
}

/// The slot is snapshotted, not held, so a nested read can't relock it.
fn write_cell(
    kind: CellKind,
    slot: &Arc<Mutex<Value>>,
    opts: &DebugOpts,
    indent: usize,
    out: &mut String,
) {
    let inner = slot.lock().clone();
    let (name, field) = match kind {
        CellKind::Rc | CellKind::Arc => {
            write_value(&inner, opts, indent, out);
            return;
        }
        CellKind::RefCell => ("RefCell", "value"),
        CellKind::Cell => ("Cell", "value"),
        CellKind::Mutex | CellKind::TokioMutex => ("Mutex", "data"),
    };
    let poisoned = kind == CellKind::Mutex;
    if opts.pretty {
        write!(out, "{name} {{\n{}{field}: ", pad(indent + 1)).unwrap();
        write_value(&inner, opts, indent + 1, out);
        out.push_str(",\n");
        if poisoned {
            write!(
                out,
                "{}poisoned: false,\n{}..\n",
                pad(indent + 1),
                pad(indent + 1)
            )
            .unwrap();
        }
        out.push_str(&pad(indent));
        out.push('}');
    } else {
        write!(out, "{name} {{ {field}: ").unwrap();
        write_value(&inner, opts, indent, out);
        out.push_str(if poisoned {
            ", poisoned: false, .. }"
        } else {
            " }"
        });
    }
}
