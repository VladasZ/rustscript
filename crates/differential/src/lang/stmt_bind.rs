//! Renders the statements that bind through a pattern, `if let`, `while let`, `let else`, a
//! `match` statement and a `loop` that breaks with a value.

use std::collections::BTreeSet;

use crate::lang::pat::Pat;
use crate::lang::ty::Ty;

use super::stmt::{ChainLink, Exit, Stmt, StmtArm};

impl Stmt {
    pub(super) fn render_binding_form(&self, mutable: &BTreeSet<String>, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        match self {
            Self::IfLet {
                links,
                then_body,
                else_body,
            } => {
                let chain: Vec<String> = links
                    .iter()
                    .map(|link| match link {
                        ChainLink::Cond(expr) => expr.render(),
                        ChainLink::Let { pat, expr } => {
                            format!("let {} = {}", pat.render_grouped(), expr.render())
                        }
                    })
                    .collect();
                let mut out = format!("{pad}if {} {{\n", chain.join(" && "));
                out.push_str(&render_body(then_body, mutable, indent + 1));
                if let Some(else_body) = else_body {
                    out.push_str(&format!("{pad}}} else {{\n"));
                    out.push_str(&render_body(else_body, mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            Self::WhileLet {
                name,
                pat,
                body,
                label,
            } => {
                let prefix = label
                    .as_deref()
                    .map(|l| format!("'{l}: "))
                    .unwrap_or_default();
                let mut out = format!(
                    "{pad}{prefix}while let {} = {name}.pop() {{\n",
                    pat.render_grouped()
                );
                out.push_str(&render_body(body, mutable, indent + 1));
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            Self::LetElse {
                pat,
                expr,
                else_body,
                exit,
            } => {
                let mut out = format!(
                    "{pad}let {} = {} else {{\n",
                    pat.render_grouped(),
                    expr.render()
                );
                out.push_str(&render_body(else_body, mutable, indent + 1));
                out.push_str(&format!("{pad}    {};\n{pad}}};\n", render_exit(exit)));
                out
            }
            Self::Match {
                scrutinee,
                by_ref,
                arms,
            } => {
                // the scrutinee is parenthesized because a struct literal is not allowed bare
                let view = if *by_ref { ".as_slice()" } else { "" };
                let mut out = format!("{pad}match ({}){view} {{\n", scrutinee.render());
                for arm in arms {
                    out.push_str(&render_arm(arm, *by_ref, mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            Self::LetLoop {
                name,
                ty,
                counter,
                limit,
                body,
                exit,
                value,
                fallback,
            } => {
                let mut out = format!(
                    "{pad}let mut {counter}: usize = 0usize;\n{pad}let {name}: {} = loop {{\n{pad}    {counter} += 1usize;\n{pad}    if {counter} > {limit}usize {{\n{pad}        break {};\n{pad}    }}\n",
                    ty.rust(),
                    fallback.render()
                );
                out.push_str(&render_body(body, mutable, indent + 1));
                out.push_str(&format!(
                    "{pad}    if {} {{\n{pad}        break {};\n{pad}    }}\n{pad}}};\n",
                    exit.render(),
                    value.render()
                ));
                out
            }
            _ => unreachable!("render_binding_form handles the pattern statements only"),
        }
    }
}

fn render_body(body: &[Stmt], mutable: &BTreeSet<String>, indent: usize) -> String {
    body.iter()
        .map(|stmt| stmt.render(mutable, indent))
        .collect()
}

fn render_exit(exit: &Exit) -> String {
    match exit {
        Exit::Break(None) => "break".to_string(),
        Exit::Break(Some(label)) => format!("break '{label}"),
        Exit::Continue(None) => "continue".to_string(),
        Exit::Continue(Some(label)) => format!("continue '{label}"),
        Exit::Return(None) => "return".to_string(),
        Exit::Return(Some(value)) => format!("return {}", value.render()),
    }
}

fn render_arm(arm: &StmtArm, by_ref: bool, mutable: &BTreeSet<String>, indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let mut out = format!("{pad}{}", arm.pat.render());
    if let Some(guard) = &arm.guard {
        out.push_str(&format!(" if {}", guard.render()));
    }
    out.push_str(" => {\n");
    if by_ref {
        out.push_str(&slice_prologue(&arm.pat, &format!("{pad}    ")));
    }
    out.push_str(&render_body(&arm.body, mutable, indent + 1));
    out.push_str(&format!("{pad}}}\n"));
    out
}

/// Slice binds are references, the body is typed against owned values.
fn slice_prologue(pat: &Pat, pad: &str) -> String {
    let mut binds = Vec::new();
    pat.bindings(&mut binds);
    let mut out = String::new();
    for (name, ty) in &binds {
        let make = if matches!(ty, Ty::Vec(_)) && is_rest(pat, name) {
            format!("{name}.to_vec()")
        } else {
            format!("{name}.clone()")
        };
        out.push_str(&format!("{pad}let {name}: {} = {make};\n", ty.rust()));
    }
    out
}

pub(super) fn is_rest(pat: &Pat, name: &str) -> bool {
    match pat {
        Pat::Slice { rest, .. } => matches!(rest, Some(Some(rest)) if rest == name),
        _ => false,
    }
}
