//! Rendering helpers for statements, labels, closures and prints.

use super::{ClosureParam, ClosureSource, PrintForm};
use crate::lang::expr::Expr;
use crate::lang::fmt::FmtSpec;
use crate::lang::ty::Ty;

pub(super) fn label_prefix(label: Option<&str>) -> String {
    label.map(|l| format!("'{l}: ")).unwrap_or_default()
}

pub(super) fn label_suffix(label: Option<&str>) -> String {
    label.map(|l| format!(" '{l}")).unwrap_or_default()
}

pub(super) fn render_closure(
    pad: &str,
    name: &str,
    source: &ClosureSource,
    calls: &[Expr],
    mutable: bool,
) -> String {
    let mut out = match source {
        ClosureSource::Literal {
            params,
            ret,
            body,
            capture_move,
            mutates,
        } => {
            let params: Vec<String> = params.iter().map(ClosureParam::pattern).collect();
            let mutability = if *mutates || mutable { "mut " } else { "" };
            let keyword = if *capture_move { "move " } else { "" };
            let body_text = match body {
                Expr::Block { .. } => body.render(),
                other => format!("{{ {} }}", other.render()),
            };
            format!(
                "{pad}let {mutability}{name} = {keyword}|{}| -> {} {body_text};\n",
                params.join(", "),
                ret.rust()
            )
        }
        ClosureSource::Factory { fn_name, arg, .. } => {
            let mutability = if mutable { "mut " } else { "" };
            format!(
                "{pad}let {mutability}{name} = {fn_name}({});\n",
                arg.render()
            )
        }
    };
    for (index, call) in calls.iter().enumerate() {
        out.push_str(&format!(
            "{pad}println!(\"{name}_{index}: {{:?}}\", {});\n",
            call.render()
        ));
    }
    out
}

/// A map or set prints through a sorted vec, real Rust randomizes its order per process.
fn observed(expr: &Expr) -> String {
    match expr.ty() {
        Ty::Map(key, value) => format!(
            "({{ let mut diff_obs: Vec<({}, {})> = {}.into_iter().collect(); diff_obs.sort(); diff_obs }})",
            key.rust(),
            value.rust(),
            expr.render()
        ),
        Ty::Set(elem) => format!(
            "({{ let mut diff_obs: Vec<{}> = {}.into_iter().collect(); diff_obs.sort(); diff_obs }})",
            elem.rust(),
            expr.render()
        ),
        _ => expr.render(),
    }
}

pub(super) fn render_print(
    pad: &str,
    label: &str,
    expr: &Expr,
    spec: &FmtSpec,
    form: PrintForm,
) -> String {
    let value = observed(expr);
    match form {
        PrintForm::Plain => format!(
            "{pad}println!(\"{label}: {}\", {value});\n",
            spec.placeholder()
        ),
        PrintForm::Indexed => format!(
            "{pad}println!(\"{label}: {}\", {value});\n",
            spec.placeholder_for("0")
        ),
        PrintForm::Twice => format!(
            "{pad}println!(\"{label}: {} {{0:?}}\", {value});\n",
            spec.placeholder_for("0")
        ),
        PrintForm::WidthArg(width) => {
            let mut widthless = *spec;
            widthless.width = None;
            format!(
                "{pad}println!(\"{label}: {{0:{}}}\", {value}, {width}usize);\n",
                with_width(&widthless.body(), "1$")
            )
        }
        PrintForm::NamedWidth(width) => {
            let mut widthless = *spec;
            widthless.width = None;
            format!(
                "{pad}println!(\"{label}: {{:{}}}\", {value}, diff_w = {width}usize);\n",
                with_width(&widthless.body(), "diff_w$")
            )
        }
    }
}

/// The width goes after the flags and before any precision and the trait letter.
fn with_width(body: &str, width: &str) -> String {
    let split = body
        .find(['.', '?', 'x', 'X', 'o', 'b', 'e', 'E'])
        .unwrap_or(body.len());
    format!("{}{width}{}", &body[..split], &body[split..])
}
