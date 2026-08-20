//! The ratatui bridge, widget side. Constructors and builder methods keep the
//! real crate's shapes, and `Widget::render` rebuilds real ratatui widgets from
//! the script values and lets ratatui draw them into a real `Buffer`. The cells
//! come back into the script's buffer, so an interpreted script and the same
//! file compiled by cargo paint identical output.

use std::sync::Arc;

use anyhow::Result;
use anyhow::bail;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Cell;
use ratatui::widgets::Row;
use ratatui::widgets::Sparkline;
use ratatui::widgets::Table;
use ratatui::widgets::Widget;

use super::bytecode::{BuiltinId as B, MethodName, PathId as P};
use super::ratatui_bridge::border_type_value;
use super::ratatui_bridge::color_value;
use super::ratatui_bridge::constraint_value;
use super::ratatui_bridge::int_of;
use super::ratatui_bridge::items;
use super::ratatui_bridge::line_value;
use super::ratatui_bridge::modifier_value;
use super::ratatui_bridge::option_inner;
use super::ratatui_bridge::padding_value;
use super::ratatui_bridge::rect_value;
use super::ratatui_bridge::style_value;
use super::ratatui_bridge::text_of;
use super::ratatui_bridge::value_border_type;
use super::ratatui_bridge::value_color;
use super::ratatui_bridge::value_constraint;
use super::ratatui_bridge::value_line;
use super::ratatui_bridge::value_modifier;
use super::ratatui_bridge::value_padding;
use super::ratatui_bridge::value_rect;
use super::ratatui_bridge::value_span;
use super::ratatui_bridge::value_style;
use super::value::StructData;
use super::value::Value;

/// Associated functions, `Table::new`, `Block::bordered`, `Buffer::empty` and
/// the rest. Returns None when the path is not a ratatui one.
pub(super) fn ratatui_assoc(id: P, args: &[Value]) -> Option<Value> {
    let arg = |i: usize| -> Value { args.get(i).cloned().unwrap_or(Value::Unit) };
    Some(match id {
        P::StyleNew | P::StyleDefault => style_value(Style::new()),
        P::RectNew => Value::struct_of(
            "Rect",
            [
                ("x".into(), arg(0)),
                ("y".into(), arg(1)),
                ("width".into(), arg(2)),
                ("height".into(), arg(3)),
            ],
        ),
        P::PaddingNew => Value::struct_of(
            "Padding",
            [
                ("left".into(), arg(0)),
                ("right".into(), arg(1)),
                ("top".into(), arg(2)),
                ("bottom".into(), arg(3)),
            ],
        ),
        P::PaddingZero | P::PaddingZeroConst => padding_value(ratatui::widgets::Padding::ZERO),
        P::SpanRaw => Value::struct_of(
            "Span",
            [
                ("content".into(), arg(0)),
                ("style".into(), style_value(Style::new())),
            ],
        ),
        P::SpanStyled => Value::struct_of(
            "Span",
            [("content".into(), arg(0)), ("style".into(), arg(1))],
        ),
        P::LineFrom | P::LineRaw | P::LineStyled => line_from(&arg(0)),
        P::CellFrom | P::CellNew => Value::struct_of(
            "Cell",
            [
                ("content".into(), line_from(&arg(0))),
                ("style".into(), style_value(Style::new())),
            ],
        ),
        P::RowNew => Value::struct_of(
            "Row",
            [
                ("cells".into(), Value::vec(items(&arg(0)))),
                ("style".into(), style_value(Style::new())),
                ("height".into(), Value::Int(1)),
            ],
        ),
        P::TableNew => Value::struct_of(
            "Table",
            [
                ("rows".into(), Value::vec(items(&arg(0)))),
                ("widths".into(), Value::vec(items(&arg(1)))),
                ("column_spacing".into(), Value::Int(1)),
                ("block".into(), Value::none()),
                ("style".into(), style_value(Style::new())),
            ],
        ),
        P::BlockNew | P::BlockDefault => block_value(false),
        P::BlockBordered => block_value(true),
        P::SparklineDefault | P::SparklineNew => Value::struct_of(
            "Sparkline",
            [
                ("data".into(), Value::vec(Vec::new())),
                ("style".into(), style_value(Style::new())),
                ("max".into(), Value::none()),
            ],
        ),
        P::BufferEmpty => Value::struct_of(
            "Buffer",
            [
                ("area".into(), arg(0)),
                ("content".into(), Value::vec(Vec::new())),
            ],
        ),
        P::WidgetRender => return render_into(&arg(0), &arg(1), &arg(2)).ok(),
        _ => return None,
    })
}

fn block_value(bordered: bool) -> Value {
    Value::struct_of(
        "Block",
        [
            ("title".into(), Value::none()),
            ("bordered".into(), Value::Bool(bordered)),
            (
                "border_type".into(),
                border_type_value(ratatui::widgets::BorderType::Plain),
            ),
            ("border_style".into(), style_value(Style::new())),
            (
                "padding".into(),
                padding_value(ratatui::widgets::Padding::ZERO),
            ),
            ("style".into(), style_value(Style::new())),
        ],
    )
}

fn line_from(v: &Value) -> Value {
    line_value(&value_line(v))
}

/// A builder method returns a fresh value the way the real consuming builders
/// do, so a script that keeps the earlier value still sees the earlier state.
fn with(s: &StructData, field: &str, v: Value) -> Value {
    let out = Value::structure(Arc::clone(&s.shape), s.values.lock().clone());
    if let Value::Struct(data) = &out {
        data.set(field, v);
    }
    out
}

pub(super) fn style_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let arg = |i: usize| -> Value { args.get(i).cloned().unwrap_or(Value::Unit) };
    Ok(match name.id {
        B::Fg => with(s, "fg", Value::some(arg(0))),
        B::Bg => with(s, "bg", Value::some(arg(0))),
        B::AddModifier => with(s, "add_modifier", arg(0)),
        B::RemoveModifier => with(s, "sub_modifier", arg(0)),
        B::Bold => add_modifier(s, ratatui::style::Modifier::BOLD),
        B::Dim => add_modifier(s, ratatui::style::Modifier::DIM),
        B::Italic => add_modifier(s, ratatui::style::Modifier::ITALIC),
        B::Underlined => add_modifier(s, ratatui::style::Modifier::UNDERLINED),
        B::Reversed => add_modifier(s, ratatui::style::Modifier::REVERSED),
        _ => bail!("unknown method `{name}` on Style"),
    })
}

fn add_modifier(s: &StructData, m: ratatui::style::Modifier) -> Value {
    let current = s
        .get("add_modifier")
        .as_ref()
        .map_or_else(ratatui::style::Modifier::empty, value_modifier);
    with(s, "add_modifier", modifier_value(current | m))
}

pub(super) fn modifier_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let mine = value_modifier(&Value::Struct(Arc::new(StructData {
        shape: Arc::clone(&s.shape),
        values: parking_lot::Mutex::new(s.values.lock().clone()),
    })));
    let other = args
        .first()
        .map_or_else(ratatui::style::Modifier::empty, value_modifier);
    Ok(match name.id {
        B::Contains => Value::Bool(mine.contains(other)),
        B::Intersects => Value::Bool(mine.intersects(other)),
        B::IsEmpty => Value::Bool(mine.is_empty()),
        B::Bits => Value::Int(i64::from(mine.bits())),
        B::Union => modifier_value(mine | other),
        B::Difference => modifier_value(mine - other),
        _ => bail!("unknown method `{name}` on Modifier"),
    })
}

pub(super) fn span_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    Ok(match name.id {
        B::Width => {
            let span = value_span(&rebuild(s));
            Value::Int(i64::try_from(span.width()).unwrap_or(0))
        }
        B::Style => with(s, "style", args.first().cloned().unwrap_or(Value::Unit)),
        B::Content => s.get("content").unwrap_or_else(|| Value::str("")),
        _ => bail!("unknown method `{name}` on Span"),
    })
}

pub(super) fn line_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    Ok(match name.id {
        B::Width => {
            let line = value_line(&rebuild(s));
            Value::Int(i64::try_from(line.width()).unwrap_or(0))
        }
        B::Style => with(s, "style", args.first().cloned().unwrap_or(Value::Unit)),
        _ => bail!("unknown method `{name}` on Line"),
    })
}

pub(super) fn cell_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    Ok(match name.id {
        B::Style => with(s, "style", args.first().cloned().unwrap_or(Value::Unit)),
        _ => bail!("unknown method `{name}` on Cell"),
    })
}

pub(super) fn row_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Unit);
    Ok(match name.id {
        B::Style => with(s, "style", arg),
        B::Height => with(s, "height", arg),
        _ => bail!("unknown method `{name}` on Row"),
    })
}

pub(super) fn table_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Unit);
    Ok(match name.id {
        B::ColumnSpacing => with(s, "column_spacing", arg),
        B::Block => with(s, "block", Value::some(arg)),
        B::Style => with(s, "style", arg),
        B::Widths => with(s, "widths", Value::vec(items(&arg))),
        _ => bail!("unknown method `{name}` on Table"),
    })
}

pub(super) fn block_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Unit);
    Ok(match name.id {
        B::Title => with(s, "title", Value::some(line_from(&arg))),
        B::BorderType => with(s, "border_type", arg),
        B::BorderStyle => with(s, "border_style", arg),
        B::Padding => with(s, "padding", arg),
        B::Style => with(s, "style", arg),
        B::Borders => with(s, "bordered", Value::Bool(true)),
        _ => bail!("unknown method `{name}` on Block"),
    })
}

pub(super) fn sparkline_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    let arg = args.first().cloned().unwrap_or(Value::Unit);
    Ok(match name.id {
        B::Data => with(s, "data", Value::vec(items(&arg))),
        B::Style => with(s, "style", arg),
        B::Max => with(s, "max", Value::some(arg)),
        _ => bail!("unknown method `{name}` on Sparkline"),
    })
}

pub(super) fn buffer_method(s: &StructData, name: &MethodName, args: &[Value]) -> Result<Value> {
    Ok(match name.id {
        // Indexes the cell list in place. Cloning the whole buffer per lookup
        // makes drawing quadratic, and a full frame reads every cell.
        B::Cell => {
            let (x, y) = coords(args.first());
            let area = s
                .get("area")
                .as_ref()
                .map_or(Rect::new(0, 0, 0, 0), value_rect);
            match (cell_index(area, x, y), s.get("content")) {
                (Some(index), Some(Value::Vec(cells))) => {
                    let cells = cells.lock();
                    match cells.get(index) {
                        Some(cell) => Value::some(cell.clone()),
                        None => Value::none(),
                    }
                }
                _ => Value::none(),
            }
        }
        B::Area => s
            .get("area")
            .unwrap_or_else(|| rect_value(Rect::new(0, 0, 0, 0))),
        _ => bail!("unknown method `{name}` on Buffer"),
    })
}

pub(super) fn buffer_cell_method(s: &StructData, name: &MethodName) -> Result<Value> {
    Ok(match name.id {
        B::Symbol => s.get("symbol").unwrap_or_else(|| Value::str("")),
        _ => bail!("unknown method `{name}` on buffer Cell"),
    })
}

fn coords(v: Option<&Value>) -> (u16, u16) {
    let parts = v.map(items).unwrap_or_default();
    let at = |i: usize| -> u16 {
        parts
            .get(i)
            .map_or(0, |v| u16::try_from(int_of(v)).unwrap_or(0))
    };
    (at(0), at(1))
}

fn cell_index(area: Rect, x: u16, y: u16) -> Option<usize> {
    if x < area.x || y < area.y || x >= area.right() || y >= area.bottom() {
        return None;
    }
    let col = usize::from(x - area.x);
    let row = usize::from(y - area.y);
    Some(row * usize::from(area.width) + col)
}

fn rebuild(s: &StructData) -> Value {
    Value::structure(Arc::clone(&s.shape), s.values.lock().clone())
}

fn build_block(v: &Value) -> Option<Block<'static>> {
    let Value::Struct(s) = v else { return None };
    let mut block = if s.get("bordered").as_ref().map(int_of) == Some(1) {
        Block::bordered()
    } else {
        Block::new()
    };
    block = block.border_type(
        s.get("border_type")
            .as_ref()
            .map_or(ratatui::widgets::BorderType::Plain, value_border_type),
    );
    if let Some(style) = s.get("border_style") {
        block = block.border_style(value_style(&style));
    }
    if let Some(padding) = s.get("padding") {
        block = block.padding(value_padding(&padding));
    }
    if let Some(style) = s.get("style") {
        block = block.style(value_style(&style));
    }
    if let Some(title) = option_inner(s.get("title").as_ref()) {
        block = block.title(value_line(&title));
    }
    Some(block)
}

fn build_row(v: &Value) -> Row<'static> {
    let Value::Struct(s) = v else {
        return Row::new(Vec::<Cell>::new());
    };
    let cells: Vec<Cell> = s
        .get("cells")
        .as_ref()
        .map(|c| items(c).iter().map(build_cell).collect())
        .unwrap_or_default();
    let mut row = Row::new(cells);
    if let Some(style) = s.get("style") {
        row = row.style(value_style(&style));
    }
    if let Some(height) = s.get("height") {
        row = row.height(u16::try_from(int_of(&height)).unwrap_or(1));
    }
    row
}

fn build_cell(v: &Value) -> Cell<'static> {
    let Value::Struct(s) = v else {
        return Cell::from(Text::from(text_of(v)));
    };
    let line: Line<'static> = s
        .get("content")
        .as_ref()
        .map_or_else(|| Line::from(""), value_line);
    let mut cell = Cell::from(Text::from(line));
    if let Some(style) = s.get("style") {
        cell = cell.style(value_style(&style));
    }
    cell
}

fn build_table(s: &StructData) -> Table<'static> {
    let rows: Vec<Row> = s
        .get("rows")
        .as_ref()
        .map(|r| items(r).iter().map(build_row).collect())
        .unwrap_or_default();
    let widths: Vec<ratatui::layout::Constraint> = s
        .get("widths")
        .as_ref()
        .map(|w| items(w).iter().map(value_constraint).collect())
        .unwrap_or_default();
    let mut table = Table::new(rows, widths);
    if let Some(spacing) = s.get("column_spacing") {
        table = table.column_spacing(u16::try_from(int_of(&spacing)).unwrap_or(1));
    }
    if let Some(style) = s.get("style") {
        table = table.style(value_style(&style));
    }
    if let Some(block) = option_inner(s.get("block").as_ref())
        .as_ref()
        .and_then(build_block)
    {
        table = table.block(block);
    }
    table
}

fn build_sparkline(s: &StructData) -> Sparkline<'static> {
    let data: Vec<u64> = s
        .get("data")
        .as_ref()
        .map(|d| {
            items(d)
                .iter()
                .map(|v| int_of(v).max(0).cast_unsigned())
                .collect()
        })
        .unwrap_or_default();
    let mut spark = Sparkline::default().data(data);
    if let Some(style) = s.get("style") {
        spark = spark.style(value_style(&style));
    }
    if let Some(max) = option_inner(s.get("max").as_ref()) {
        spark = spark.max(int_of(&max).max(0).cast_unsigned());
    }
    spark
}

/// `Widget::render(widget, area, &mut buf)`. The script buffer is refilled
/// from a real ratatui buffer, so ratatui itself decides every cell.
fn render_into(widget: &Value, area: &Value, target: &Value) -> Result<Value> {
    let Value::Struct(buffer) = target else {
        bail!("Widget::render expects a Buffer");
    };
    let buffer_area = buffer
        .get("area")
        .as_ref()
        .map_or(Rect::new(0, 0, 0, 0), value_rect);
    let mut real = Buffer::empty(buffer_area);
    restore(&mut real, buffer_area, buffer.get("content").as_ref());

    let area = value_rect(area);
    match widget {
        Value::Struct(s) => match &**s.name() {
            "Table" => build_table(s).render(area, &mut real),
            "Sparkline" => build_sparkline(s).render(area, &mut real),
            "Block" => {
                if let Some(block) = build_block(widget) {
                    block.render(area, &mut real);
                }
            }
            other => bail!("cannot render `{other}`"),
        },
        _ => bail!("Widget::render expects a widget"),
    }

    buffer.set("content", Value::vec(dump(&real, buffer_area)));
    Ok(Value::Unit)
}

/// Cells the script already holds, so a second render draws over the first
/// instead of starting from a blank buffer.
fn restore(real: &mut Buffer, area: Rect, content: Option<&Value>) {
    let Some(cells) = content.map(items) else {
        return;
    };
    for (index, value) in cells.iter().enumerate() {
        let Value::Struct(s) = value else { continue };
        let x = area.x + u16::try_from(index % usize::from(area.width.max(1))).unwrap_or(0);
        let y = area.y + u16::try_from(index / usize::from(area.width.max(1))).unwrap_or(0);
        let Some(cell) = real.cell_mut((x, y)) else {
            continue;
        };
        if let Some(symbol) = s.get("symbol") {
            cell.set_symbol(&text_of(&symbol));
        }
        if let Some(fg) = s.get("fg") {
            cell.fg = value_color(&fg);
        }
        if let Some(bg) = s.get("bg") {
            cell.bg = value_color(&bg);
        }
        if let Some(modifier) = s.get("modifier") {
            cell.modifier = value_modifier(&modifier);
        }
    }
}

fn dump(real: &Buffer, area: Rect) -> Vec<Value> {
    let mut out = Vec::with_capacity(usize::from(area.width) * usize::from(area.height));
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let Some(cell) = real.cell((x, y)) else {
                continue;
            };
            out.push(Value::struct_of(
                "BufferCell",
                [
                    ("symbol".into(), Value::str(cell.symbol())),
                    ("fg".into(), color_value(cell.fg)),
                    ("bg".into(), color_value(cell.bg)),
                    ("modifier".into(), modifier_value(cell.modifier)),
                ],
            ));
        }
    }
    out
}

/// The constraint constructors, `Constraint::Length(10)` and friends.
pub(super) fn constraint_variant(id: P, args: &[Value]) -> Option<Value> {
    let n = args.first().map_or(0, int_of);
    let second = args.get(1).map_or(0, int_of);
    let c = match id {
        P::ConstraintLength => ratatui::layout::Constraint::Length(u16::try_from(n).unwrap_or(0)),
        P::ConstraintMin => ratatui::layout::Constraint::Min(u16::try_from(n).unwrap_or(0)),
        P::ConstraintMax => ratatui::layout::Constraint::Max(u16::try_from(n).unwrap_or(0)),
        P::ConstraintPercentage => {
            ratatui::layout::Constraint::Percentage(u16::try_from(n).unwrap_or(0))
        }
        P::ConstraintFill => ratatui::layout::Constraint::Fill(u16::try_from(n).unwrap_or(0)),
        P::ConstraintRatio => ratatui::layout::Constraint::Ratio(
            u32::try_from(n).unwrap_or(0),
            u32::try_from(second).unwrap_or(1),
        ),
        _ => return None,
    };
    Some(constraint_value(c))
}

/// `Color::Rgb(r, g, b)` and `Color::Indexed(i)`, the two colour variants that
/// carry data.
pub(super) fn color_variant(id: P, args: &[Value]) -> Option<Value> {
    let at = |i: usize| -> u8 {
        args.get(i)
            .map_or(0, |v| u8::try_from(int_of(v)).unwrap_or(0))
    };
    match id {
        P::ColorRgb => Some(color_value(ratatui::style::Color::Rgb(at(0), at(1), at(2)))),
        P::ColorIndexed => Some(color_value(ratatui::style::Color::Indexed(at(0)))),
        _ => None,
    }
}
