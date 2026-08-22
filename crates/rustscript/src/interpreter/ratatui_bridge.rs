//! The ratatui bridge, value side. Every type is a plain data struct with
//! the real fields, nothing renders here, see `ratatui_render`.

use num_traits::AsPrimitive;
use ratatui::layout::Constraint;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::BorderType;
use ratatui::widgets::Padding;

use super::bytecode::PathId;
use super::enum_def::{BORDER_TYPE, COLOR, CONSTRAINT, EnumKind};
use super::value::Value;

/// `Modifier::BOLD` and friends. The colour and border constants are enum
/// variants.
pub(super) fn ratatui_const(id: PathId) -> Option<Value> {
    modifier_const(id).map(modifier_value)
}

fn modifier_const(id: PathId) -> Option<Modifier> {
    Some(match id {
        PathId::ModifierBold => Modifier::BOLD,
        PathId::ModifierDim => Modifier::DIM,
        PathId::ModifierItalic => Modifier::ITALIC,
        PathId::ModifierUnderlined => Modifier::UNDERLINED,
        PathId::ModifierSlowBlink => Modifier::SLOW_BLINK,
        PathId::ModifierRapidBlink => Modifier::RAPID_BLINK,
        PathId::ModifierReversed => Modifier::REVERSED,
        PathId::ModifierHidden => Modifier::HIDDEN,
        PathId::ModifierCrossedOut => Modifier::CROSSED_OUT,
        _ => return None,
    })
}

fn color_const(name: &str) -> Option<Color> {
    Some(match name {
        "Reset" => Color::Reset,
        "Black" => Color::Black,
        "Red" => Color::Red,
        "Green" => Color::Green,
        "Yellow" => Color::Yellow,
        "Blue" => Color::Blue,
        "Magenta" => Color::Magenta,
        "Cyan" => Color::Cyan,
        "Gray" => Color::Gray,
        "DarkGray" => Color::DarkGray,
        "LightRed" => Color::LightRed,
        "LightGreen" => Color::LightGreen,
        "LightYellow" => Color::LightYellow,
        "LightBlue" => Color::LightBlue,
        "LightMagenta" => Color::LightMagenta,
        "LightCyan" => Color::LightCyan,
        "White" => Color::White,
        _ => return None,
    })
}

fn border_type_const(name: &str) -> Option<BorderType> {
    Some(match name {
        "Plain" => BorderType::Plain,
        "Rounded" => BorderType::Rounded,
        "Double" => BorderType::Double,
        "Thick" => BorderType::Thick,
        "LightDoubleDashed" => BorderType::LightDoubleDashed,
        "HeavyDoubleDashed" => BorderType::HeavyDoubleDashed,
        "LightTripleDashed" => BorderType::LightTripleDashed,
        "HeavyTripleDashed" => BorderType::HeavyTripleDashed,
        "LightQuadrupleDashed" => BorderType::LightQuadrupleDashed,
        "HeavyQuadrupleDashed" => BorderType::HeavyQuadrupleDashed,
        "QuadrantInside" => BorderType::QuadrantInside,
        "QuadrantOutside" => BorderType::QuadrantOutside,
        _ => return None,
    })
}

fn border_type_name(b: BorderType) -> &'static str {
    match b {
        BorderType::Plain => "Plain",
        BorderType::Rounded => "Rounded",
        BorderType::Double => "Double",
        BorderType::Thick => "Thick",
        BorderType::LightDoubleDashed => "LightDoubleDashed",
        BorderType::HeavyDoubleDashed => "HeavyDoubleDashed",
        BorderType::LightTripleDashed => "LightTripleDashed",
        BorderType::HeavyTripleDashed => "HeavyTripleDashed",
        BorderType::LightQuadrupleDashed => "LightQuadrupleDashed",
        BorderType::HeavyQuadrupleDashed => "HeavyQuadrupleDashed",
        BorderType::QuadrantInside => "QuadrantInside",
        BorderType::QuadrantOutside => "QuadrantOutside",
    }
}

pub(super) fn modifier_value(m: Modifier) -> Value {
    Value::struct_of(
        "Modifier",
        [("bits".into(), Value::Int(i64::from(m.bits())))],
    )
}

pub(super) fn value_modifier(v: &Value) -> Modifier {
    let bits = match v {
        Value::Struct(s) => s.get("bits").as_ref().map_or(0, int_of),
        other => int_of(other),
    };
    u16::try_from(bits)
        .ok()
        .and_then(Modifier::from_bits)
        .unwrap_or_else(Modifier::empty)
}

pub(super) fn color_value(c: Color) -> Value {
    let (variant, data): (&str, Vec<Value>) = match c {
        Color::Reset => ("Reset", Vec::new()),
        Color::Black => ("Black", Vec::new()),
        Color::Red => ("Red", Vec::new()),
        Color::Green => ("Green", Vec::new()),
        Color::Yellow => ("Yellow", Vec::new()),
        Color::Blue => ("Blue", Vec::new()),
        Color::Magenta => ("Magenta", Vec::new()),
        Color::Cyan => ("Cyan", Vec::new()),
        Color::Gray => ("Gray", Vec::new()),
        Color::DarkGray => ("DarkGray", Vec::new()),
        Color::LightRed => ("LightRed", Vec::new()),
        Color::LightGreen => ("LightGreen", Vec::new()),
        Color::LightYellow => ("LightYellow", Vec::new()),
        Color::LightBlue => ("LightBlue", Vec::new()),
        Color::LightMagenta => ("LightMagenta", Vec::new()),
        Color::LightCyan => ("LightCyan", Vec::new()),
        Color::White => ("White", Vec::new()),
        Color::Rgb(r, g, b) => (
            "Rgb",
            vec![
                Value::Int(i64::from(r)),
                Value::Int(i64::from(g)),
                Value::Int(i64::from(b)),
            ],
        ),
        Color::Indexed(i) => ("Indexed", vec![Value::Int(i64::from(i))]),
    };
    Value::enum_named(&COLOR, variant, data).expect("every ratatui color variant is listed")
}

pub(super) fn value_color(v: &Value) -> Color {
    let Value::Enum { def, variant, data } = v else {
        return Color::Reset;
    };
    let variant = def.variant_name(*variant);
    let data = data.lock().clone();
    let at = |i: usize| -> u8 {
        data.get(i)
            .map_or(0, |v| u8::try_from(int_of(v)).unwrap_or(0))
    };
    match &**variant {
        "Rgb" => Color::Rgb(at(0), at(1), at(2)),
        "Indexed" => Color::Indexed(at(0)),
        name => color_const(name).unwrap_or(Color::Reset),
    }
}

pub(super) fn style_value(s: Style) -> Value {
    Value::struct_of(
        "Style",
        [
            (
                "fg".into(),
                s.fg.map_or_else(Value::none, |c| Value::some(color_value(c))),
            ),
            (
                "bg".into(),
                s.bg.map_or_else(Value::none, |c| Value::some(color_value(c))),
            ),
            ("add_modifier".into(), modifier_value(s.add_modifier)),
            ("sub_modifier".into(), modifier_value(s.sub_modifier)),
        ],
    )
}

pub(super) fn value_style(v: &Value) -> Style {
    let Value::Struct(s) = v else {
        return Style::new();
    };
    let mut style = Style::new();
    if let Some(c) = option_inner(s.get("fg").as_ref()) {
        style = style.fg(value_color(&c));
    }
    if let Some(c) = option_inner(s.get("bg").as_ref()) {
        style = style.bg(value_color(&c));
    }
    if let Some(m) = s.get("add_modifier") {
        style = style.add_modifier(value_modifier(&m));
    }
    if let Some(m) = s.get("sub_modifier") {
        style = style.remove_modifier(value_modifier(&m));
    }
    style
}

pub(super) fn rect_value(r: Rect) -> Value {
    Value::struct_of(
        "Rect",
        [
            ("x".into(), Value::Int(i64::from(r.x))),
            ("y".into(), Value::Int(i64::from(r.y))),
            ("width".into(), Value::Int(i64::from(r.width))),
            ("height".into(), Value::Int(i64::from(r.height))),
        ],
    )
}

pub(super) fn value_rect(v: &Value) -> Rect {
    let Value::Struct(s) = v else {
        return Rect::new(0, 0, 0, 0);
    };
    let field = |name: &str| -> u16 {
        s.get(name)
            .as_ref()
            .map_or(0, |v| u16::try_from(int_of(v)).unwrap_or(0))
    };
    Rect::new(field("x"), field("y"), field("width"), field("height"))
}

pub(super) fn span_value(span: &Span) -> Value {
    Value::struct_of(
        "Span",
        [
            ("content".into(), Value::str(span.content.to_string())),
            ("style".into(), style_value(span.style)),
        ],
    )
}

pub(super) fn value_span(v: &Value) -> Span<'static> {
    match v {
        Value::Struct(s) if &**s.name() == "Span" => {
            let content = s.get("content").as_ref().map(text_of).unwrap_or_default();
            let style = s.get("style").as_ref().map_or_else(Style::new, value_style);
            Span::styled(content, style)
        }
        other => Span::raw(text_of(other)),
    }
}

pub(super) fn line_value(line: &Line) -> Value {
    let spans: Vec<Value> = line.spans.iter().map(span_value).collect();
    Value::struct_of(
        "Line",
        [
            ("spans".into(), Value::vec(spans)),
            ("style".into(), style_value(line.style)),
        ],
    )
}

pub(super) fn value_line(v: &Value) -> Line<'static> {
    match v {
        Value::Struct(s) if &**s.name() == "Line" => {
            let spans = s
                .get("spans")
                .as_ref()
                .map(|v| {
                    items(v)
                        .iter()
                        .map(value_span)
                        .collect::<Vec<Span<'static>>>()
                })
                .unwrap_or_default();
            let style = s.get("style").as_ref().map_or_else(Style::new, value_style);
            Line::from(spans).style(style)
        }
        Value::Vec(_) => Line::from(items(v).iter().map(value_span).collect::<Vec<Span>>()),
        other => Line::from(value_span(other)),
    }
}

pub(super) fn constraint_value(c: Constraint) -> Value {
    let (variant, data): (&str, Vec<Value>) = match c {
        Constraint::Length(n) => ("Length", vec![Value::Int(i64::from(n))]),
        Constraint::Min(n) => ("Min", vec![Value::Int(i64::from(n))]),
        Constraint::Max(n) => ("Max", vec![Value::Int(i64::from(n))]),
        Constraint::Percentage(n) => ("Percentage", vec![Value::Int(i64::from(n))]),
        Constraint::Fill(n) => ("Fill", vec![Value::Int(i64::from(n))]),
        Constraint::Ratio(a, b) => (
            "Ratio",
            vec![Value::Int(i64::from(a)), Value::Int(i64::from(b))],
        ),
    };
    Value::enum_named(&CONSTRAINT, variant, data)
        .expect("every ratatui constraint variant is listed")
}

pub(super) fn value_constraint(v: &Value) -> Constraint {
    let Value::Enum { def, variant, data } = v else {
        return Constraint::Min(0);
    };
    let variant = def.variant_name(*variant);
    let data = data.lock().clone();
    let at = |i: usize| -> u16 {
        data.get(i)
            .map_or(0, |v| u16::try_from(int_of(v)).unwrap_or(0))
    };
    match &**variant {
        "Length" => Constraint::Length(at(0)),
        "Max" => Constraint::Max(at(0)),
        "Percentage" => Constraint::Percentage(at(0)),
        "Fill" => Constraint::Fill(at(0)),
        "Ratio" => Constraint::Ratio(u32::from(at(0)), u32::from(at(1))),
        _ => Constraint::Min(at(0)),
    }
}

pub(super) fn border_type_value(b: BorderType) -> Value {
    Value::enum_named(&BORDER_TYPE, border_type_name(b), Vec::new())
        .expect("every ratatui border type is listed")
}

pub(super) fn value_border_type(v: &Value) -> BorderType {
    match v {
        Value::Enum { def, variant, .. } => {
            border_type_const(def.variant_name(*variant)).unwrap_or(BorderType::Plain)
        }
        _ => BorderType::Plain,
    }
}

pub(super) fn padding_value(p: Padding) -> Value {
    Value::struct_of(
        "Padding",
        [
            ("left".into(), Value::Int(i64::from(p.left))),
            ("right".into(), Value::Int(i64::from(p.right))),
            ("top".into(), Value::Int(i64::from(p.top))),
            ("bottom".into(), Value::Int(i64::from(p.bottom))),
        ],
    )
}

pub(super) fn value_padding(v: &Value) -> Padding {
    let Value::Struct(s) = v else {
        return Padding::ZERO;
    };
    let field = |name: &str| -> u16 {
        s.get(name)
            .as_ref()
            .map_or(0, |v| u16::try_from(int_of(v)).unwrap_or(0))
    };
    Padding::new(field("left"), field("right"), field("top"), field("bottom"))
}

/// So a script may pass a `u16` field or a plain literal to the same
/// helper.
pub(super) fn int_of(v: &Value) -> i64 {
    match v {
        Value::Int(i) | Value::IntW(i, _) => *i,
        Value::Bool(b) => i64::from(*b),
        Value::Float(f) => AsPrimitive::<i64>::as_(*f),
        _ => 0,
    }
}

pub(super) fn text_of(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Int(i) | Value::IntW(i, _) => i.to_string(),
        _ => String::new(),
    }
}

pub(super) fn items(v: &Value) -> Vec<Value> {
    match v {
        Value::Vec(items) | Value::Tuple(items) => items.lock().clone(),
        _ => Vec::new(),
    }
}

pub(super) fn option_inner(v: Option<&Value>) -> Option<Value> {
    let v = v?;
    if v.is_enum_kind(EnumKind::Option) {
        v.some_payload()
    } else {
        Some(v.clone())
    }
}
