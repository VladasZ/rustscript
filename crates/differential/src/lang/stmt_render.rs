//! Renders a `Stmt` to Rust source text, prints included.

use std::collections::BTreeSet;

use crate::lang::expr::Expr;
use crate::lang::fmt::FmtSpec;
use crate::lang::ty::Ty;

use super::stmt::{Ann, ClosureParam, ClosureSource, PrintForm, Stmt};

impl Stmt {
    pub fn render(&self, mutable: &BTreeSet<String>, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        match self {
            Self::Let { .. } | Self::LetTuple { .. } => self.render_binding(mutable, &pad),
            Self::LetClosure {
                name,
                source,
                calls,
            } => render_closure(&pad, name, source, calls, mutable.contains(name)),
            Self::Assign { name, expr } => format!("{pad}{name} = {};\n", expr.render()),
            Self::AssignField { .. } | Self::Swap { .. } | Self::Scope { .. } => {
                self.render_place_write(mutable, indent, &pad)
            }
            Self::Compound { name, op, expr } => {
                // `String += &str`, every other compound takes the value
                let rhs = if matches!(expr.ty(), Ty::Str) {
                    format!("&{}", expr.render())
                } else {
                    expr.render()
                };
                format!("{pad}{name} {}= {rhs};\n", op.token())
            }
            Self::Print {
                label,
                expr,
                spec,
                form,
            } => render_print(&pad, label, expr, spec, *form),
            Self::Mutate { name, op } => format!("{pad}{}\n", op.render(name)),
            Self::ForAccum {
                var,
                source,
                target,
                op,
            } => format!(
                "{pad}for {var} in {} {{\n{pad}    {}\n{pad}}}\n",
                source.render(),
                op.render(target)
            ),
            Self::ForMut {
                name,
                var,
                elem,
                expr,
            } => format!(
                "{pad}for diff_ref in {name}.iter_mut() {{\n{pad}    let {var}: {} = diff_ref.clone();\n{pad}    *diff_ref = {};\n{pad}}}\n",
                elem.rust(),
                expr.render()
            ),
            Self::CallMut {
                name,
                fn_name,
                args,
            } => {
                let mut rendered = vec![format!("&mut {name}")];
                rendered.extend(args.iter().map(Expr::render));
                format!("{pad}{fn_name}({});\n", rendered.join(", "))
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                let mut out = format!("{pad}if {} {{\n", condition.render());
                for stmt in then_body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}} else {{\n"));
                for stmt in else_body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            Self::ForRange { .. } | Self::While { .. } | Self::Loop { .. } => {
                self.render_loop(mutable, indent, &pad)
            }
            Self::Break { condition, label } => {
                format!(
                    "{pad}if {} {{\n{pad}    break{};\n{pad}}}\n",
                    condition.render(),
                    label_suffix(label.as_deref())
                )
            }
            Self::Continue { condition, label } => {
                format!(
                    "{pad}if {} {{\n{pad}    continue{};\n{pad}}}\n",
                    condition.render(),
                    label_suffix(label.as_deref())
                )
            }
            Self::Return { condition, value } => format!(
                "{pad}if {} {{\n{pad}    return {};\n{pad}}}\n",
                condition.render(),
                value.render()
            ),
        }
    }

    fn render_place_write(&self, mutable: &BTreeSet<String>, indent: usize, pad: &str) -> String {
        match self {
            Self::AssignField {
                name,
                base,
                index,
                expr,
            } => format!(
                "{pad}{name}.{} = {};\n",
                field_name(base, *index),
                expr.render()
            ),
            Self::Swap { a, b } => format!("{pad}std::mem::swap(&mut {a}, &mut {b});\n"),
            Self::Scope { body } => {
                let mut out = format!("{pad}{{\n");
                for stmt in body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            _ => unreachable!("render_place_write handles the place writes only"),
        }
    }

    fn render_loop(&self, mutable: &BTreeSet<String>, indent: usize, pad: &str) -> String {
        match self {
            Self::ForRange {
                var,
                count,
                body,
                label,
            } => {
                let mut out = format!(
                    "{pad}{}for {var} in 0usize..{count}usize {{\n",
                    label_prefix(label.as_deref())
                );
                for stmt in body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            Self::While {
                counter,
                limit,
                body,
                label,
            }
            | Self::Loop {
                counter,
                limit,
                body,
                label,
            } => {
                let prefix = label_prefix(label.as_deref());
                let head = if matches!(self, Self::While { .. }) {
                    format!(
                        "{pad}let mut {counter}: usize = 0usize;\n{pad}{prefix}while {counter} < {limit}usize {{\n{pad}    {counter} += 1usize;\n"
                    )
                } else {
                    format!(
                        "{pad}let mut {counter}: usize = 0usize;\n{pad}{prefix}loop {{\n{pad}    {counter} += 1usize;\n{pad}    if {counter} > {limit}usize {{\n{pad}        break;\n{pad}    }}\n"
                    )
                };
                let mut out = head;
                for stmt in body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
            _ => unreachable!("render_loop handles the loops only"),
        }
    }

    fn render_binding(&self, mutable: &BTreeSet<String>, pad: &str) -> String {
        match self {
            Self::Let {
                name,
                ty,
                expr,
                ann,
                mutable: own_mutable,
            } => {
                let mutability = if *own_mutable { "mut " } else { "" };
                let annotation = match ann {
                    Ann::Typed => format!(": {}", ty.rust()),
                    Ann::Inferred => String::new(),
                };
                format!(
                    "{pad}let {mutability}{name}{annotation} = {};\n",
                    expr.render()
                )
            }
            Self::LetTuple { names, expr, ann } => {
                let pattern: Vec<String> = names
                    .iter()
                    .map(|(name, _)| {
                        if mutable.contains(name) {
                            format!("mut {name}")
                        } else {
                            name.clone()
                        }
                    })
                    .collect();
                let annotation = match ann {
                    Ann::Typed => {
                        let tys: Vec<String> = names.iter().map(|(_, ty)| ty.rust()).collect();
                        format!(": ({})", tys.join(", "))
                    }
                    Ann::Inferred => String::new(),
                };
                format!(
                    "{pad}let ({}){annotation} = {};\n",
                    pattern.join(", "),
                    expr.render()
                )
            }
            _ => unreachable!("render_binding handles the let forms only"),
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        for (body_index, body) in self.bodies().iter().enumerate() {
            for index in 0..body.len() {
                let mut candidate = self.clone();
                if let Some(target) = candidate.bodies_mut().into_iter().nth(body_index) {
                    *target = crate::lang::block::remove_with_dependents(body, index);
                }
                candidates.push(candidate);
            }
        }
        if let Self::ForRange { count, .. } = self
            && *count > 1
        {
            let mut candidate = self.clone();
            if let Self::ForRange { count, .. } = &mut candidate {
                *count = 1;
            }
            candidates.push(candidate);
        }
        if let Self::LetClosure { calls, .. } = self
            && calls.len() > 1
        {
            let mut candidate = self.clone();
            if let Self::LetClosure { calls, .. } = &mut candidate {
                calls.pop();
            }
            candidates.push(candidate);
        }
        let expr_count = self.exprs().len();
        for index in 0..expr_count {
            for shrunk in self.exprs()[index].shrinks() {
                let mut candidate = self.clone();
                if let Some(slot) = candidate.exprs_mut().into_iter().nth(index) {
                    *slot = shrunk;
                }
                candidates.push(candidate);
            }
        }
        candidates
    }
}

/// `f0` on a struct, `0` on a tuple.
pub fn field_name(base: &Ty, index: usize) -> String {
    match base {
        Ty::User(shape) => shape.fields()[index].name.clone(),
        _ => index.to_string(),
    }
}

/// Sets `mutable` on every `let` that a later write resolves to. A shadowed name may be written
/// in one scope and not in another, so a flat name set would mark both. `tail` is the value
/// expression after the statements, a fn body's last line.
pub fn mark_mutable(stmts: &mut [Stmt], tail: Option<&Expr>) {
    let mut written: Vec<Vec<usize>> = Vec::new();
    let mut scope: Vec<(String, Vec<usize>)> = Vec::new();
    collect_written(stmts, &mut Vec::new(), &mut scope, &mut written);
    if let Some(tail) = tail {
        let mut names = BTreeSet::new();
        tail.written_names(&mut names);
        for node in tail.nodes() {
            if let Expr::Block { stmts, .. } = node {
                for stmt in stmts {
                    stmt.assigned(&mut names);
                }
            }
        }
        for name in names {
            if let Some((_, let_path)) = scope.iter().rev().find(|(bound, _)| *bound == name) {
                written.push(let_path.clone());
            }
        }
    }
    for path in written {
        if let Some(Stmt::Let { mutable, .. }) = stmt_at_mut(stmts, &path) {
            *mutable = true;
        }
    }
}

fn collect_written(
    stmts: &[Stmt],
    path: &mut Vec<usize>,
    scope: &mut Vec<(String, Vec<usize>)>,
    written: &mut Vec<Vec<usize>>,
) {
    for (index, stmt) in stmts.iter().enumerate() {
        path.push(index);
        for name in stmt.own_writes() {
            if let Some((_, let_path)) = scope.iter().rev().find(|(bound, _)| *bound == name) {
                written.push(let_path.clone());
            }
        }
        for (body_index, body) in stmt.bodies().iter().enumerate() {
            path.push(body_index);
            let mark = scope.len();
            collect_written(body, path, scope, written);
            scope.truncate(mark);
            path.pop();
        }
        if let Stmt::Let { name, .. } = stmt {
            scope.push((name.clone(), path.clone()));
        }
        path.pop();
    }
}

/// `path` alternates a statement index and a body index, see `collect_written`.
fn stmt_at_mut<'a>(stmts: &'a mut [Stmt], path: &[usize]) -> Option<&'a mut Stmt> {
    let (&index, rest) = path.split_first()?;
    let stmt = stmts.get_mut(index)?;
    if rest.is_empty() {
        return Some(stmt);
    }
    let (&body_index, rest) = rest.split_first()?;
    let body = stmt.bodies_mut().into_iter().nth(body_index)?;
    stmt_at_mut(body, rest)
}

fn label_prefix(label: Option<&str>) -> String {
    label.map(|l| format!("'{l}: ")).unwrap_or_default()
}

fn label_suffix(label: Option<&str>) -> String {
    label.map(|l| format!(" '{l}")).unwrap_or_default()
}

fn render_closure(
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

/// A map or set prints through a sorted vec, real Rust randomizes its order per process. A plain
/// binding is printed by reference, `println!` never takes its argument.
fn observed(expr: &Expr) -> String {
    if let Expr::Var { name, ty, .. } = expr
        && !matches!(ty, Ty::Map(..) | Ty::Set(_))
    {
        return name.clone();
    }
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

fn render_print(pad: &str, label: &str, expr: &Expr, spec: &FmtSpec, form: PrintForm) -> String {
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
