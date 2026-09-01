//! Statements. Every observation is a labeled print, so a mismatch names the line that produced it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{BinOp, Expr, Helper};
use crate::lang::fmt::FmtSpec;
use crate::lang::ty::Ty;

mod mut_op;
mod parts;
mod render;

pub use mut_op::MutOp;
pub use parts::{Ann, ClosureParam, ClosureSource, PrintForm};
use render::{label_prefix, label_suffix, render_closure, render_print};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        expr: Expr,
        ann: Ann,
    },
    /// `let (a, b) = tuple;`
    LetTuple {
        names: Vec<(String, Ty)>,
        expr: Expr,
        ann: Ann,
    },
    /// A closure bound by `let`. Its calls follow at once when it borrows mutably, so the borrow
    /// ends before anything else reads the binding.
    LetClosure {
        name: String,
        source: ClosureSource,
        /// the calls printed right after the binding
        calls: Vec<Expr>,
    },
    Assign {
        name: String,
        expr: Expr,
    },
    /// `name op= expr;`
    Compound {
        name: String,
        op: BinOp,
        expr: Expr,
    },
    Print {
        label: String,
        expr: Expr,
        spec: FmtSpec,
        form: PrintForm,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    ForRange {
        var: String,
        count: usize,
        body: Vec<Stmt>,
        /// `'label: for`, only when a `break` or `continue` in the body names it
        #[serde(default)]
        label: Option<String>,
    },
    /// The counter is incremented first so a `continue` can't loop forever.
    While {
        counter: String,
        limit: u8,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    Loop {
        counter: String,
        limit: u8,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { break 'label; }`, inside a loop body only
    Break {
        condition: Expr,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { continue 'label; }`, inside a loop body only
    Continue {
        condition: Expr,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { return value; }`, inside a function body only
    Return {
        condition: Expr,
        value: Expr,
    },
    Mutate {
        name: String,
        op: MutOp,
    },
    /// `for var in source { accumulate into target }`. The source must have a defined order, so
    /// it is always a vec.
    ForAccum {
        var: String,
        source: Expr,
        target: String,
        op: MutOp,
    },
    /// `for r in name.iter_mut() { let var: T = r.clone(); *r = expr; }`
    ForMut {
        name: String,
        var: String,
        elem: Ty,
        expr: Expr,
    },
    /// `fn_name(&mut name, args);`, a helper that writes through a `&mut`
    CallMut {
        name: String,
        fn_name: String,
        args: Vec<Expr>,
    },
}

impl Stmt {
    /// Whether a `break` or `continue` in here, at any depth, names `label`.
    pub fn targets_label(&self, label: &str) -> bool {
        match self {
            Self::Break { label: Some(l), .. } | Self::Continue { label: Some(l), .. } => {
                l == label
            }
            _ => self
                .bodies()
                .iter()
                .any(|body| body.iter().any(|stmt| stmt.targets_label(label))),
        }
    }

    pub fn bodies(&self) -> Vec<&Vec<Stmt>> {
        match self {
            Self::If {
                then_body,
                else_body,
                ..
            } => vec![then_body, else_body],
            Self::ForRange { body, .. } | Self::While { body, .. } | Self::Loop { body, .. } => {
                vec![body]
            }
            _ => Vec::new(),
        }
    }

    fn bodies_mut(&mut self) -> Vec<&mut Vec<Stmt>> {
        match self {
            Self::If {
                then_body,
                else_body,
                ..
            } => vec![then_body, else_body],
            Self::ForRange { body, .. } | Self::While { body, .. } | Self::Loop { body, .. } => {
                vec![body]
            }
            _ => Vec::new(),
        }
    }

    /// Every expression, in a fixed order shared with `exprs_mut`.
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        match self {
            Self::Let { expr, .. }
            | Self::LetTuple { expr, .. }
            | Self::Assign { expr, .. }
            | Self::Compound { expr, .. }
            | Self::Print { expr, .. }
            | Self::ForMut { expr, .. } => out.push(expr),
            Self::LetClosure { source, calls, .. } => {
                match source {
                    ClosureSource::Literal { body, .. } => out.push(body),
                    ClosureSource::Factory { arg, .. } => out.push(arg),
                }
                out.extend(calls.iter());
            }
            Self::If { condition, .. }
            | Self::Break { condition, .. }
            | Self::Continue { condition, .. } => out.push(condition),
            Self::Return { condition, value } => {
                out.push(condition);
                out.push(value);
            }
            Self::Mutate { op, .. } => out.extend(op.exprs()),
            Self::ForAccum { source, op, .. } => {
                out.push(source);
                out.extend(op.exprs());
            }
            Self::CallMut { args, .. } => out.extend(args.iter()),
            Self::ForRange { .. } | Self::While { .. } | Self::Loop { .. } => {}
        }
        for body in self.bodies() {
            for stmt in body {
                out.extend(stmt.exprs());
            }
        }
        out
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        match self {
            Self::Let { expr, .. }
            | Self::LetTuple { expr, .. }
            | Self::Assign { expr, .. }
            | Self::Compound { expr, .. }
            | Self::Print { expr, .. }
            | Self::ForMut { expr, .. } => out.push(expr),
            Self::LetClosure { source, calls, .. } => {
                match source {
                    ClosureSource::Literal { body, .. } => out.push(body),
                    ClosureSource::Factory { arg, .. } => out.push(arg),
                }
                out.extend(calls.iter_mut());
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                out.push(condition);
                for stmt in then_body.iter_mut().chain(else_body.iter_mut()) {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::Break { condition, .. } | Self::Continue { condition, .. } => {
                out.push(condition);
            }
            Self::Return { condition, value } => {
                out.push(condition);
                out.push(value);
            }
            Self::Mutate { op, .. } => out.extend(op.exprs_mut()),
            Self::ForAccum { source, op, .. } => {
                out.push(source);
                out.extend(op.exprs_mut());
            }
            Self::CallMut { args, .. } => out.extend(args.iter_mut()),
            Self::ForRange { body, .. } | Self::While { body, .. } | Self::Loop { body, .. } => {
                for stmt in body {
                    out.extend(stmt.exprs_mut());
                }
            }
        }
        out
    }

    /// Names this statement writes to, so the renderer knows which bindings need `mut`.
    pub fn assigned(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Assign { name, .. }
            | Self::Compound { name, .. }
            | Self::Mutate { name, .. }
            | Self::ForMut { name, .. }
            | Self::CallMut { name, .. }
            | Self::ForAccum { target: name, .. }
            | Self::LetClosure {
                name,
                source: ClosureSource::Literal { mutates: true, .. },
                ..
            } => {
                out.insert(name.clone());
            }
            _ => {}
        }
        for body in self.bodies() {
            for stmt in body {
                stmt.assigned(out);
            }
        }
        for expr in self.exprs() {
            for node in expr.nodes() {
                match node {
                    Expr::Block { stmts, .. } => {
                        for stmt in stmts {
                            stmt.assigned(out);
                        }
                    }
                    // the apply helper takes the closure by `&mut`
                    Expr::ApplyCall { closure, .. } => {
                        out.insert(closure.clone());
                    }
                    _ => {}
                }
            }
        }
    }

    /// The binding a compound assignment writes.
    pub fn declared_targets(&self) -> Vec<String> {
        match self {
            Self::Compound { name, .. } | Self::Assign { name, .. } => vec![name.clone()],
            _ => Vec::new(),
        }
    }

    pub fn declared(&self) -> Vec<String> {
        match self {
            Self::Let { name, .. } | Self::LetClosure { name, .. } => vec![name.clone()],
            Self::LetTuple { names, .. } => names.iter().map(|(n, _)| n.clone()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        let direct = match self {
            Self::LetClosure {
                source: ClosureSource::Factory { fn_name, .. },
                ..
            } => names.contains(fn_name),
            _ => false,
        };
        direct || self.exprs().iter().any(|expr| expr.uses_any(names))
    }

    /// Whether this statement assigns to a name it doesn't own, so the reducer can't drop that binding.
    pub fn writes_any(&self, names: &BTreeSet<String>) -> bool {
        let mut written = BTreeSet::new();
        self.assigned(&mut written);
        written.iter().any(|name| names.contains(name))
    }

    pub fn has_fallible_op(&self) -> bool {
        let own = match self {
            Self::Compound { op, .. } => op.is_fallible(),
            Self::Mutate { op, .. } | Self::ForAccum { op, .. } => op.has_fallible_op(),
            _ => false,
        };
        own || self.exprs().iter().any(|expr| expr.has_fallible_op())
    }

    pub fn make_opaque(&mut self) {
        for expr in self.exprs_mut() {
            expr.make_opaque();
        }
    }

    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        for expr in self.exprs() {
            expr.helpers(out);
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        let own: &[&'static str] = match self {
            Self::Let {
                ann: Ann::Typed, ..
            } => &["lang-let"],
            Self::Let {
                ann: Ann::Inferred, ..
            } => &["lang-let", "lang-let-inferred"],
            Self::LetTuple { .. } => &["lang-let-tuple"],
            Self::LetClosure { source, .. } => match source {
                ClosureSource::Literal {
                    capture_move: true,
                    mutates: true,
                    ..
                } => &["lang-closure", "lang-closure-move", "lang-closure-mut"],
                ClosureSource::Literal {
                    capture_move: true, ..
                } => &["lang-closure", "lang-closure-move"],
                ClosureSource::Literal { mutates: true, .. } => {
                    &["lang-closure", "lang-closure-mut"]
                }
                ClosureSource::Literal { .. } => &["lang-closure"],
                ClosureSource::Factory { .. } => &["lang-closure", "lang-closure-factory"],
            },
            Self::Assign { .. } => &["lang-assign"],
            Self::Compound { .. } => &["lang-compound"],
            Self::Print { .. } | Self::Mutate { .. } => &[],
            Self::If { .. } => &["lang-if-stmt"],
            Self::ForRange { label: Some(_), .. } => &["lang-for", "lang-loop-label"],
            Self::ForRange { .. } => &["lang-for"],
            Self::While { label: Some(_), .. } => &["lang-while", "lang-loop-label"],
            Self::While { .. } => &["lang-while"],
            Self::Loop { label: Some(_), .. } => &["lang-loop", "lang-loop-label"],
            Self::Loop { .. } => &["lang-loop"],
            Self::Break { label: Some(_), .. } => &["lang-break", "lang-break-label"],
            Self::Break { .. } => &["lang-break"],
            Self::Continue { label: Some(_), .. } => &["lang-continue", "lang-continue-label"],
            Self::Continue { .. } => &["lang-continue"],
            Self::Return { .. } => &["lang-early-return"],
            Self::ForAccum { .. } => &["lang-for-accum"],
            Self::ForMut { .. } => &["lang-iter-mut"],
            Self::CallMut { .. } => &["lang-borrow-mut"],
        };
        out.extend(own.iter().copied());
        if let Self::LetClosure {
            source: ClosureSource::Literal { params, .. },
            ..
        } = self
            && params
                .iter()
                .any(|param| matches!(param, ClosureParam::Pair { .. }))
        {
            out.insert("lang-closure-tuple-param");
        }
        match self {
            Self::Print { spec, form, .. } => {
                spec.features(out);
                out.insert(form.feature());
            }
            Self::Compound { op, .. } => {
                out.insert(op.feature());
            }
            Self::Mutate { op, .. } | Self::ForAccum { op, .. } => {
                out.insert(op.feature());
            }
            _ => {}
        }
        for body in self.bodies() {
            for stmt in body {
                stmt.features(out);
            }
        }
        for expr in self.exprs() {
            expr.features(out);
        }
    }

    pub fn shape(&self, out: &mut String) {
        match self {
            Self::Let { .. } => out.push_str("let,"),
            Self::LetTuple { .. } => out.push_str("let-tuple,"),
            Self::LetClosure { .. } => out.push_str("closure,"),
            Self::Assign { .. } => out.push_str("assign,"),
            Self::Compound { .. } => out.push_str("compound,"),
            Self::Print { .. } => out.push_str("print,"),
            Self::If {
                then_body,
                else_body,
                ..
            } => {
                out.push_str("if(");
                for stmt in then_body {
                    stmt.shape(out);
                }
                out.push('|');
                for stmt in else_body {
                    stmt.shape(out);
                }
                out.push_str("),");
            }
            Self::ForRange { body, .. } | Self::While { body, .. } | Self::Loop { body, .. } => {
                out.push_str(match self {
                    Self::ForRange { .. } => "for(",
                    Self::While { .. } => "while(",
                    _ => "loop(",
                });
                for stmt in body {
                    stmt.shape(out);
                }
                out.push_str("),");
            }
            Self::Break { .. } => out.push_str("break,"),
            Self::Continue { .. } => out.push_str("continue,"),
            Self::Return { .. } => out.push_str("return,"),
            Self::Mutate { op, .. } => {
                out.push_str("mutate:");
                out.push_str(op.feature());
                out.push(',');
            }
            Self::ForAccum { op, .. } => {
                out.push_str("for-accum:");
                out.push_str(op.feature());
                out.push(',');
            }
            Self::ForMut { .. } => out.push_str("for-mut,"),
            Self::CallMut { .. } => out.push_str("call-mut,"),
        }
    }

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
            } => {
                let mutability = if mutable.contains(name) { "mut " } else { "" };
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
                    target.remove(index);
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
