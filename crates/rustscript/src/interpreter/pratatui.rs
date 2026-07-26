//! The ratatui bridge for the parallel engine. The widget values are plain
//! data trees, so instead of a second copy of every conversion this converts
//! the tree to the fast engine's `Value`, reuses the one real rendering path,
//! and converts the result back. One implementation decides how a widget draws,
//! so a `#[tokio::main]` script cannot drift from a plain one.

use anyhow::Result;

use super::pvalue::PStructData;
use super::pvalue::PValue;
use super::value::Value;

/// Names the bridge owns. Checked before conversion so an unrelated struct
/// never pays for a round trip.
pub(super) fn is_ratatui_struct(name: &str) -> bool {
    matches!(
        name,
        "Style"
            | "Modifier"
            | "Span"
            | "Line"
            | "Cell"
            | "Row"
            | "Table"
            | "Block"
            | "Sparkline"
            | "Buffer"
            | "BufferCell"
            | "Rect"
            | "Padding"
    )
}

pub(super) fn ratatui_const(ty: &str, name: &str) -> Option<PValue> {
    super::ratatui_bridge::ratatui_const(ty, name).map(|v| from_value(&v))
}

pub(super) fn ratatui_assoc(ty: &str, func: &str, args: &[PValue]) -> Option<PValue> {
    let converted: Vec<Value> = args.iter().map(to_value).collect();
    let out = super::ratatui_render::ratatui_assoc(ty, func, &converted)
        .or_else(|| super::ratatui_render::constraint_variant(ty, func, &converted))
        .or_else(|| super::ratatui_render::color_variant(ty, func, &converted))?;
    // `Widget::render` writes into the buffer it was handed, and the converted
    // copy is what it wrote into, so the cells are copied back to the caller's
    // own buffer value here.
    if ty == "Widget"
        && func == "render"
        && let Some(PValue::Struct(target)) = args.get(2)
        && let Some(Value::Struct(source)) = converted.get(2)
        && let Some(content) = source.get("content")
    {
        target.set("content", from_value(&content));
    }
    Some(from_value(&out))
}

/// Reading a cell goes straight at the parallel value. Converting the whole
/// buffer for every lookup would make drawing one frame quadratic in its cell
/// count, and a frame reads every cell it drew.
fn buffer_cell(st: &PStructData, args: &[PValue]) -> Option<PValue> {
    let PValue::Struct(area) = st.get("area")? else {
        return None;
    };
    let coord = |name: &str| -> u16 {
        area.get(name)
            .map(|v| match v {
                PValue::Int(i) => i,
                PValue::IntW(i, _) => i,
                _ => 0,
            })
            .and_then(|i| u16::try_from(i).ok())
            .unwrap_or(0)
    };
    let PValue::Tuple(point) = args.first()? else {
        return None;
    };
    let point = point.lock();
    let at = |i: usize| -> u16 {
        point
            .get(i)
            .map(|v| match v {
                PValue::Int(n) => *n,
                PValue::IntW(n, _) => *n,
                _ => 0,
            })
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(0)
    };
    let (x, y) = (at(0), at(1));
    let (ax, ay, width, height) = (coord("x"), coord("y"), coord("width"), coord("height"));
    if x < ax || y < ay || x >= ax + width || y >= ay + height {
        return Some(PValue::none());
    }
    let index = usize::from(y - ay) * usize::from(width) + usize::from(x - ax);
    let PValue::Vec(cells) = st.get("content")? else {
        return None;
    };
    let cells = cells.lock();
    Some(match cells.get(index) {
        Some(cell) => PValue::some(cell.clone()),
        None => PValue::none(),
    })
}

pub(super) fn struct_method(st: &PStructData, m: &str, args: &[PValue]) -> Result<PValue> {
    if &**st.name() == "Buffer"
        && m == "cell"
        && let Some(found) = buffer_cell(st, args)
    {
        return Ok(found);
    }
    let recv = to_value(&PValue::Struct(std::sync::Arc::new(PStructData {
        shape: std::sync::Arc::clone(&st.shape),
        values: parking_lot::Mutex::new(st.values.lock().clone()),
    })));
    let Value::Struct(data) = &recv else {
        anyhow::bail!("ratatui method on a non struct");
    };
    let converted: Vec<Value> = args.iter().map(to_value).collect();
    let out = match &**st.name() {
        "Style" => super::ratatui_render::style_method(data, m, &converted),
        "Modifier" => super::ratatui_render::modifier_method(data, m, &converted),
        "Span" => super::ratatui_render::span_method(data, m, &converted),
        "Line" => super::ratatui_render::line_method(data, m, &converted),
        "Cell" => super::ratatui_render::cell_method(data, m, &converted),
        "Row" => super::ratatui_render::row_method(data, m, &converted),
        "Table" => super::ratatui_render::table_method(data, m, &converted),
        "Block" => super::ratatui_render::block_method(data, m, &converted),
        "Sparkline" => super::ratatui_render::sparkline_method(data, m, &converted),
        "Buffer" => super::ratatui_render::buffer_method(data, m, &converted),
        "BufferCell" => super::ratatui_render::buffer_cell_method(data, m),
        other => anyhow::bail!("unknown method `{m}` on {other}"),
    }?;
    Ok(from_value(&out))
}

/// Parallel value to fast value, for the plain data the widgets are made of.
fn to_value(p: &PValue) -> Value {
    match p {
        PValue::Unit => Value::Unit,
        PValue::Bool(b) => Value::Bool(*b),
        PValue::Int(i) => Value::Int(*i),
        PValue::IntW(i, w) => Value::IntW(*i, *w),
        PValue::Float(f) => Value::Float(*f),
        PValue::F32(f) => Value::F32(*f),
        PValue::Char(c) => Value::Char(*c),
        PValue::Str(s) => Value::str(s.to_string()),
        PValue::Vec(items) => Value::vec(items.lock().iter().map(to_value).collect()),
        PValue::Tuple(items) => Value::Tuple(std::rc::Rc::new(std::cell::RefCell::new(
            items.lock().iter().map(to_value).collect(),
        ))),
        PValue::Struct(st) => {
            let pairs: Vec<(std::rc::Rc<str>, Value)> = st
                .shape
                .fields
                .iter()
                .map(|f| {
                    let v = st.get(f).map(|v| to_value(&v)).unwrap_or(Value::Unit);
                    (std::rc::Rc::from(&**f), v)
                })
                .collect();
            Value::struct_of(&*st.shape.name, pairs)
        }
        PValue::Enum {
            enum_name,
            variant,
            data,
        } => Value::Enum {
            enum_name: std::rc::Rc::from(&**enum_name),
            variant: std::rc::Rc::from(&**variant),
            data: data.iter().map(to_value).collect(),
        },
        _ => Value::Unit,
    }
}

/// Fast value back to parallel value.
fn from_value(v: &Value) -> PValue {
    match v {
        Value::Unit => PValue::Unit,
        Value::Bool(b) => PValue::Bool(*b),
        Value::Int(i) => PValue::Int(*i),
        Value::IntW(i, w) => PValue::IntW(*i, *w),
        Value::Float(f) => PValue::Float(*f),
        Value::F32(f) => PValue::F32(*f),
        Value::Char(c) => PValue::Char(*c),
        Value::Str(s) => PValue::str(s.as_str()),
        Value::Vec(items) => PValue::vec(items.borrow().iter().map(from_value).collect()),
        Value::Tuple(items) => PValue::Tuple(std::sync::Arc::new(parking_lot::Mutex::new(
            items.borrow().iter().map(from_value).collect(),
        ))),
        Value::Struct(st) => {
            let pairs: Vec<(std::sync::Arc<str>, PValue)> = st
                .shape
                .fields
                .iter()
                .map(|f| {
                    let value = st.get(f).map(|v| from_value(&v)).unwrap_or(PValue::Unit);
                    (std::sync::Arc::from(&**f), value)
                })
                .collect();
            PValue::struct_of(&*st.shape.name, pairs)
        }
        Value::Enum {
            enum_name,
            variant,
            data,
        } => PValue::Enum {
            enum_name: std::sync::Arc::from(&**enum_name),
            variant: std::sync::Arc::from(&**variant),
            data: data.iter().map(from_value).collect(),
        },
        _ => PValue::Unit,
    }
}
