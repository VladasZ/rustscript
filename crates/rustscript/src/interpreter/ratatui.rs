//! Router for the ratatui bridge. The value side lives in `ratatui_bridge`,
//! the widget side in `ratatui_render`.

use anyhow::Result;

use super::bytecode::{MethodName, PathId};
use super::value::{StructData, Value};

/// Checked before dispatch so an unrelated struct never routes here.
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

pub(super) fn ratatui_const(id: PathId) -> Option<Value> {
    super::ratatui_bridge::ratatui_const(id)
}

pub(super) fn ratatui_assoc(id: PathId, args: &[Value]) -> Option<Value> {
    super::ratatui_render::ratatui_assoc(id, args)
        .or_else(|| super::ratatui_render::constraint_variant(id, args))
        .or_else(|| super::ratatui_render::color_variant(id, args))
}

pub(super) fn struct_method(st: &StructData, m: &MethodName, args: &[Value]) -> Result<Value> {
    match &**st.name() {
        "Style" => super::ratatui_render::style_method(st, m, args),
        "Modifier" => super::ratatui_render::modifier_method(st, m, args),
        "Span" => super::ratatui_render::span_method(st, m, args),
        "Line" => super::ratatui_render::line_method(st, m, args),
        "Cell" => super::ratatui_render::cell_method(st, m, args),
        "Row" => super::ratatui_render::row_method(st, m, args),
        "Table" => super::ratatui_render::table_method(st, m, args),
        "Block" => super::ratatui_render::block_method(st, m, args),
        "Sparkline" => super::ratatui_render::sparkline_method(st, m, args),
        "Buffer" => super::ratatui_render::buffer_method(st, m, args),
        "BufferCell" => super::ratatui_render::buffer_cell_method(st, m),
        other => anyhow::bail!("unknown method `{m}` on {other}"),
    }
}
