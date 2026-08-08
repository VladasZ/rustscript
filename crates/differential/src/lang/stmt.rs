//! Statements and blocks. A generated block is a real `fn main` body: typed
//! bindings, reassignment, nested control flow, and a labeled print per
//! observation so a mismatch names the line that produced it.

use std::collections::BTreeSet;
use std::mem::take;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{Expr, Helper};
use crate::lang::ty::Ty;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        expr: Expr,
    },
    Assign {
        name: String,
        expr: Expr,
    },
    Print {
        label: String,
        expr: Expr,
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
    },
    /// An in-place mutation of a collection binding.
    Mutate {
        name: String,
        op: MutOp,
    },
    /// `for var in source { accum-mutation on target }`, the loop-accumulation
    /// shape scripts build maps and vecs with. The source must iterate in a
    /// defined order, so generation only feeds it vecs.
    ForAccum {
        var: String,
        source: Expr,
        target: String,
        op: MutOp,
    },
}

/// One in-place operation on a collection binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutOp {
    VecPush(Expr),
    VecSort,
    VecDedup,
    /// `name[i] = value;`, which panics out of bounds exactly like debug Rust.
    VecSetIndex {
        index: u8,
        value: Expr,
    },
    VecExtend(Expr),
    MapInsert {
        key: Expr,
        value: Expr,
    },
    MapRemove {
        key: Expr,
    },
    /// `*name.entry(key).or_insert(default) += add;`, integer values only.
    MapEntryAdd {
        key: Expr,
        default: Expr,
        add: Expr,
    },
    /// `name.entry(key).or_default().push(value);`, vec values only.
    MapEntryPush {
        key: Expr,
        value: Expr,
    },
    SetInsert(Expr),
    SetRemove(Expr),
}

impl MutOp {
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::VecPush(expr)
            | Self::VecExtend(expr)
            | Self::SetInsert(expr)
            | Self::SetRemove(expr) => vec![expr],
            Self::VecSort | Self::VecDedup => Vec::new(),
            Self::VecSetIndex { value, .. } => vec![value],
            Self::MapInsert { key, value } | Self::MapEntryPush { key, value } => vec![key, value],
            Self::MapRemove { key } => vec![key],
            Self::MapEntryAdd { key, default, add } => vec![key, default, add],
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::VecPush(expr)
            | Self::VecExtend(expr)
            | Self::SetInsert(expr)
            | Self::SetRemove(expr) => vec![expr],
            Self::VecSort | Self::VecDedup => Vec::new(),
            Self::VecSetIndex { value, .. } => vec![value],
            Self::MapInsert { key, value } | Self::MapEntryPush { key, value } => vec![key, value],
            Self::MapRemove { key } => vec![key],
            Self::MapEntryAdd { key, default, add } => vec![key, default, add],
        }
    }

    fn render(&self, name: &str) -> String {
        match self {
            Self::VecPush(expr) => format!("{name}.push({});", expr.render()),
            Self::VecSort => format!("{name}.sort();"),
            Self::VecDedup => format!("{name}.dedup();"),
            Self::VecSetIndex { index, value } => {
                format!("{name}[{index}usize] = {};", value.render())
            }
            Self::VecExtend(expr) => format!("{name}.extend({});", expr.render()),
            Self::MapInsert { key, value } => {
                format!("{name}.insert({}, {});", key.render(), value.render())
            }
            Self::MapRemove { key } => format!("{name}.remove(&{});", key.render()),
            Self::MapEntryAdd { key, default, add } => format!(
                "*{name}.entry({}).or_insert({}) += {};",
                key.render(),
                default.render(),
                add.render()
            ),
            Self::MapEntryPush { key, value } => format!(
                "{name}.entry({}).or_default().push({});",
                key.render(),
                value.render()
            ),
            Self::SetInsert(expr) => format!("{name}.insert({});", expr.render()),
            Self::SetRemove(expr) => format!("{name}.remove(&{});", expr.render()),
        }
    }

    /// `+=` on an entry can overflow, everything else only moves values.
    fn has_fallible_op(&self) -> bool {
        matches!(self, Self::MapEntryAdd { .. } | Self::VecSetIndex { .. })
            || self.exprs().iter().any(|expr| expr.has_fallible_op())
    }

    fn feature(&self) -> &'static str {
        match self {
            Self::VecPush(_) => "lang-mut-push",
            Self::VecSort => "lang-mut-sort",
            Self::VecDedup => "lang-mut-dedup",
            Self::VecSetIndex { .. } => "lang-mut-index-write",
            Self::VecExtend(_) => "lang-mut-extend",
            Self::MapInsert { .. } => "lang-mut-map-insert",
            Self::MapRemove { .. } => "lang-mut-map-remove",
            Self::MapEntryAdd { .. } => "lang-mut-entry-add",
            Self::MapEntryPush { .. } => "lang-mut-entry-push",
            Self::SetInsert(_) => "lang-mut-set-insert",
            Self::SetRemove(_) => "lang-mut-set-remove",
        }
    }
}

impl Stmt {
    /// Names this statement writes to, including inside nested blocks, so the
    /// renderer knows which bindings need `mut`.
    pub fn assigned(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Assign { name, .. } | Self::Mutate { name, .. } => {
                out.insert(name.clone());
            }
            Self::ForAccum { target, .. } => {
                out.insert(target.clone());
            }
            Self::If {
                then_body,
                else_body,
                ..
            } => {
                then_body.iter().for_each(|stmt| stmt.assigned(out));
                else_body.iter().for_each(|stmt| stmt.assigned(out));
            }
            Self::ForRange { body, .. } => body.iter().for_each(|stmt| stmt.assigned(out)),
            _ => {}
        }
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } | Self::Print { expr, .. } => {
                expr.uses_any(names)
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                condition.uses_any(names)
                    || then_body.iter().any(|stmt| stmt.uses_any(names))
                    || else_body.iter().any(|stmt| stmt.uses_any(names))
            }
            Self::ForRange { body, .. } => body.iter().any(|stmt| stmt.uses_any(names)),
            Self::Mutate { op, .. } => op.exprs().iter().any(|expr| expr.uses_any(names)),
            Self::ForAccum { source, op, .. } => {
                source.uses_any(names) || op.exprs().iter().any(|expr| expr.uses_any(names))
            }
        }
    }

    /// Whether this statement assigns to a name it does not own, which makes
    /// it invalid once that binding is dropped by the reducer.
    pub fn writes_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Assign { name, .. } | Self::Mutate { name, .. } => names.contains(name),
            Self::ForAccum { target, .. } => names.contains(target),
            Self::If {
                then_body,
                else_body,
                ..
            } => {
                then_body.iter().any(|stmt| stmt.writes_any(names))
                    || else_body.iter().any(|stmt| stmt.writes_any(names))
            }
            Self::ForRange { body, .. } => body.iter().any(|stmt| stmt.writes_any(names)),
            _ => false,
        }
    }

    pub fn has_fallible_op(&self) -> bool {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } | Self::Print { expr, .. } => {
                expr.has_fallible_op()
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                condition.has_fallible_op()
                    || then_body.iter().any(Stmt::has_fallible_op)
                    || else_body.iter().any(Stmt::has_fallible_op)
            }
            Self::ForRange { body, .. } => body.iter().any(Stmt::has_fallible_op),
            Self::Mutate { op, .. } => op.has_fallible_op(),
            Self::ForAccum { source, op, .. } => source.has_fallible_op() || op.has_fallible_op(),
        }
    }

    pub fn make_opaque(&mut self) {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } | Self::Print { expr, .. } => {
                expr.make_opaque();
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                condition.make_opaque();
                then_body.iter_mut().for_each(Stmt::make_opaque);
                else_body.iter_mut().for_each(Stmt::make_opaque);
            }
            Self::ForRange { body, .. } => body.iter_mut().for_each(Stmt::make_opaque),
            Self::Mutate { op, .. } => op.exprs_mut().into_iter().for_each(Expr::make_opaque),
            Self::ForAccum { source, op, .. } => {
                source.make_opaque();
                op.exprs_mut().into_iter().for_each(Expr::make_opaque);
            }
        }
    }

    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } | Self::Print { expr, .. } => {
                expr.helpers(out);
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                condition.helpers(out);
                then_body.iter().for_each(|stmt| stmt.helpers(out));
                else_body.iter().for_each(|stmt| stmt.helpers(out));
            }
            Self::ForRange { body, .. } => body.iter().for_each(|stmt| stmt.helpers(out)),
            Self::Mutate { op, .. } => op.exprs().iter().for_each(|expr| expr.helpers(out)),
            Self::ForAccum { source, op, .. } => {
                source.helpers(out);
                op.exprs().iter().for_each(|expr| expr.helpers(out));
            }
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        match self {
            Self::Let { expr, .. } => {
                out.insert("lang-let");
                expr.features(out);
            }
            Self::Assign { expr, .. } => {
                out.insert("lang-assign");
                expr.features(out);
            }
            Self::Print { expr, .. } => expr.features(out),
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                out.insert("lang-if-stmt");
                condition.features(out);
                then_body.iter().for_each(|stmt| stmt.features(out));
                else_body.iter().for_each(|stmt| stmt.features(out));
            }
            Self::ForRange { body, .. } => {
                out.insert("lang-for");
                body.iter().for_each(|stmt| stmt.features(out));
            }
            Self::Mutate { op, .. } => {
                out.insert(op.feature());
                op.exprs().iter().for_each(|expr| expr.features(out));
            }
            Self::ForAccum { source, op, .. } => {
                out.insert("lang-for-accum");
                out.insert(op.feature());
                source.features(out);
                op.exprs().iter().for_each(|expr| expr.features(out));
            }
        }
    }

    pub fn shape(&self, out: &mut String) {
        match self {
            Self::Let { .. } => out.push_str("let,"),
            Self::Assign { .. } => out.push_str("assign,"),
            Self::Print { .. } => out.push_str("print,"),
            Self::If {
                then_body,
                else_body,
                ..
            } => {
                out.push_str("if(");
                then_body.iter().for_each(|stmt| stmt.shape(out));
                out.push('|');
                else_body.iter().for_each(|stmt| stmt.shape(out));
                out.push_str("),");
            }
            Self::ForRange { body, .. } => {
                out.push_str("for(");
                body.iter().for_each(|stmt| stmt.shape(out));
                out.push_str("),");
            }
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
        }
    }

    pub fn render(&self, mutable: &BTreeSet<String>, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        match self {
            Self::Let { name, ty, expr } => {
                let mutability = if mutable.contains(name) { "mut " } else { "" };
                format!(
                    "{pad}let {mutability}{name}: {} = {};\n",
                    ty.rust(),
                    expr.render()
                )
            }
            Self::Assign { name, expr } => format!("{pad}{name} = {};\n", expr.render()),
            // A map or set prints through a sorted vec, never raw: real Rust
            // randomizes its iteration order per process, so a raw print would
            // flag a fake divergence on nearly every run.
            Self::Print { label, expr } => match expr.ty() {
                Ty::Map(key, value) => format!(
                    "{pad}println!(\"{label}: {{:?}}\", ({{ let mut diff_obs: Vec<({}, {})> = {}.into_iter().collect(); diff_obs.sort(); diff_obs }}));\n",
                    key.rust(),
                    value.rust(),
                    expr.render()
                ),
                Ty::Set(elem) => format!(
                    "{pad}println!(\"{label}: {{:?}}\", ({{ let mut diff_obs: Vec<{}> = {}.into_iter().collect(); diff_obs.sort(); diff_obs }}));\n",
                    elem.rust(),
                    expr.render()
                ),
                _ => format!("{pad}println!(\"{label}: {{:?}}\", {});\n", expr.render()),
            },
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
            Self::ForRange { var, count, body } => {
                let mut out = format!("{pad}for {var} in 0usize..{count}usize {{\n");
                for stmt in body {
                    out.push_str(&stmt.render(mutable, indent + 1));
                }
                out.push_str(&format!("{pad}}}\n"));
                out
            }
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        match self {
            Self::Let { name, ty, expr } => expr
                .shrinks()
                .into_iter()
                .map(|expr| Self::Let {
                    name: name.clone(),
                    ty: ty.clone(),
                    expr,
                })
                .collect(),
            Self::Assign { name, expr } => expr
                .shrinks()
                .into_iter()
                .map(|expr| Self::Assign {
                    name: name.clone(),
                    expr,
                })
                .collect(),
            Self::Print { label, expr } => expr
                .shrinks()
                .into_iter()
                .map(|expr| Self::Print {
                    label: label.clone(),
                    expr,
                })
                .collect(),
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                let mut candidates = Vec::new();
                for index in 0..then_body.len() {
                    let mut shorter = then_body.clone();
                    shorter.remove(index);
                    candidates.push(Self::If {
                        condition: condition.clone(),
                        then_body: shorter,
                        else_body: else_body.clone(),
                    });
                }
                for index in 0..else_body.len() {
                    let mut shorter = else_body.clone();
                    shorter.remove(index);
                    candidates.push(Self::If {
                        condition: condition.clone(),
                        then_body: then_body.clone(),
                        else_body: shorter,
                    });
                }
                candidates
            }
            Self::ForRange { var, count, body } => {
                let mut candidates = Vec::new();
                if *count > 1 {
                    candidates.push(Self::ForRange {
                        var: var.clone(),
                        count: 1,
                        body: body.clone(),
                    });
                }
                for index in 0..body.len() {
                    let mut shorter = body.clone();
                    shorter.remove(index);
                    candidates.push(Self::ForRange {
                        var: var.clone(),
                        count: *count,
                        body: shorter,
                    });
                }
                candidates
            }
            Self::Mutate { name, op } => {
                let mut candidates = Vec::new();
                for (index, expr) in op.exprs().iter().enumerate() {
                    for shrunk in expr.shrinks() {
                        let mut smaller = op.clone();
                        if let Some(slot) = smaller.exprs_mut().into_iter().nth(index) {
                            *slot = shrunk;
                        }
                        candidates.push(Self::Mutate {
                            name: name.clone(),
                            op: smaller,
                        });
                    }
                }
                candidates
            }
            Self::ForAccum {
                var,
                source,
                target,
                op,
            } => {
                let mut candidates: Vec<Self> = source
                    .shrinks()
                    .into_iter()
                    .map(|shrunk| Self::ForAccum {
                        var: var.clone(),
                        source: shrunk,
                        target: target.clone(),
                        op: op.clone(),
                    })
                    .collect();
                for (index, expr) in op.exprs().iter().enumerate() {
                    for shrunk in expr.shrinks() {
                        let mut smaller = op.clone();
                        if let Some(slot) = smaller.exprs_mut().into_iter().nth(index) {
                            *slot = shrunk;
                        }
                        candidates.push(Self::ForAccum {
                            var: var.clone(),
                            source: source.clone(),
                            target: target.clone(),
                            op: smaller,
                        });
                    }
                }
                candidates
            }
        }
    }

    /// Whether any expression in the statement calls the helper `name`.
    fn calls_fn(&self, name: &str) -> bool {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } | Self::Print { expr, .. } => {
                expr.calls_fn(name)
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                condition.calls_fn(name)
                    || then_body.iter().any(|stmt| stmt.calls_fn(name))
                    || else_body.iter().any(|stmt| stmt.calls_fn(name))
            }
            Self::ForRange { body, .. } => body.iter().any(|stmt| stmt.calls_fn(name)),
            Self::Mutate { op, .. } => op.exprs().iter().any(|expr| expr.calls_fn(name)),
            Self::ForAccum { source, op, .. } => {
                source.calls_fn(name) || op.exprs().iter().any(|expr| expr.calls_fn(name))
            }
        }
    }
}

/// A generated zero-argument helper function. Its return type is the only
/// place the target of a bare `collect` in its body is written down, which is
/// exactly the inference site being hunted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub ret: Ty,
    pub body: Expr,
}

impl FnDef {
    pub fn render(&self) -> String {
        format!(
            "fn {}() -> {} {{\n    {}\n}}\n\n",
            self.name,
            self.ret.rust(),
            self.body.render()
        )
    }
}

/// One generated program body, plus the helper functions its bindings call.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub statements: Vec<Stmt>,
    #[serde(default)]
    pub fns: Vec<FnDef>,
}

impl Block {
    pub fn mutable_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for stmt in &self.statements {
            stmt.assigned(&mut out);
        }
        out
    }

    pub fn render(&self) -> String {
        let mutable = self.mutable_names();
        let mut out = String::new();
        for stmt in &self.statements {
            out.push_str(&stmt.render(&mutable, 1));
        }
        out
    }

    /// The generated helper functions, rendered above `fn main`.
    pub fn render_fns(&self) -> String {
        self.fns.iter().map(FnDef::render).collect()
    }

    pub fn helpers(&self) -> BTreeSet<Helper> {
        let mut out = BTreeSet::new();
        for stmt in &self.statements {
            stmt.helpers(&mut out);
        }
        for def in &self.fns {
            def.body.helpers(&mut out);
        }
        out
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        for stmt in &self.statements {
            stmt.features(out);
        }
        for def in &self.fns {
            out.insert("lang-fn-def");
            def.body.features(out);
        }
    }

    pub fn shape(&self, out: &mut String) {
        out.push_str("lang[");
        for stmt in &self.statements {
            stmt.shape(out);
        }
        for def in &self.fns {
            out.push_str("fn:");
            out.push_str(&def.ret.rust());
            out.push(',');
        }
        out.push(']');
    }

    /// Every integer literal is laundered as soon as the block contains
    /// anything that can abort, so an overflow stays a runtime panic that the
    /// harness can compare instead of a compile error that wastes the case.
    pub fn seal(&mut self) {
        let fallible = self.statements.iter().any(Stmt::has_fallible_op)
            || self.fns.iter().any(|def| def.body.has_fallible_op());
        if fallible {
            self.statements.iter_mut().for_each(Stmt::make_opaque);
            self.fns.iter_mut().for_each(|def| def.body.make_opaque());
        }
    }

    /// Drop helper functions no statement calls anymore, so a shrunk program
    /// never carries an orphan definition.
    fn retain_called_fns(&mut self) {
        let statements = take(&mut self.statements);
        self.fns
            .retain(|def| statements.iter().any(|stmt| stmt.calls_fn(&def.name)));
        self.statements = statements;
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        for index in 0..self.statements.len() {
            if let Some(candidate) = self.without(index) {
                candidates.push(candidate);
            }
            for stmt in self.statements[index].shrinks() {
                let mut candidate = self.clone();
                candidate.statements[index] = stmt;
                candidate.seal();
                candidates.push(candidate);
            }
        }
        for index in 0..self.fns.len() {
            for body in self.fns[index].body.shrinks() {
                let mut candidate = self.clone();
                candidate.fns[index].body = body;
                candidate.seal();
                candidates.push(candidate);
            }
        }
        candidates
    }

    /// Drop one statement, and everything that depended on it, so the result
    /// still compiles.
    fn without(&self, index: usize) -> Option<Self> {
        let mut dropped: BTreeSet<String> = BTreeSet::new();
        if let Stmt::Let { name, .. } = &self.statements[index] {
            dropped.insert(name.clone());
        }
        let mut statements = Vec::new();
        for (position, stmt) in self.statements.iter().enumerate() {
            if position == index {
                continue;
            }
            if stmt.uses_any(&dropped) || stmt.writes_any(&dropped) {
                if let Stmt::Let { name, .. } = stmt {
                    dropped.insert(name.clone());
                }
                continue;
            }
            statements.push(stmt.clone());
        }
        let mut candidate = Self {
            statements,
            fns: self.fns.clone(),
        };
        candidate.retain_called_fns();
        candidate.seal();
        Some(candidate)
    }
}
