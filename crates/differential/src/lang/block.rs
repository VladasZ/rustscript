//! A generated program body with the items it needs above `main`: user
//! types, consts, and helper functions.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{BinOp, Expr, Helper};
use crate::lang::stmt::{ClosureSource, Stmt};
use crate::lang::ty::Ty;
use crate::lang::user::UserDef;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ParamMode {
    Owned,
    /// `&T`, cloned into an owned local first thing in the body.
    Ref,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
    pub mode: ParamMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FnKind {
    /// A body over owned and borrowed parameters. The return type is the
    /// only place a bare `collect` or `sum` in the body states its target,
    /// and a `Result` return is where `?` converts through `From`.
    Plain {
        params: Vec<Param>,
        ret: Ty,
        body: Expr,
    },
    /// `fn name(target: &mut T, params..) { *target = value; }`, the value
    /// reading the old target through `diff_cur`.
    Writer {
        target: Ty,
        params: Vec<Param>,
        value: Expr,
    },
    /// `fn name<T: Clone + Debug>(a: T, b: T, first: bool) -> T`.
    GenericPick,
    /// `fn name<F: FnMut(T) -> T>(f: &mut F, x: T) -> T { f(x) }`.
    Apply { ty: Ty },
    /// `fn name(n: T) -> impl Fn(T) -> T { move |x: T| (x op n) }`.
    Factory { ty: Ty, op: BinOp },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub kind: FnKind,
}

impl FnDef {
    pub fn render(&self) -> String {
        match &self.kind {
            FnKind::Plain { params, ret, body } => {
                let (sig, prologue) = render_params(params);
                let body_text = match body {
                    Expr::Block { stmts, tail } => {
                        let mut out = String::new();
                        for stmt in stmts {
                            out.push_str(&stmt.render(&block_mutable(stmts), 1));
                        }
                        out.push_str(&format!("    {}\n", tail.render()));
                        out
                    }
                    other => format!("    {}\n", other.render()),
                };
                format!(
                    "fn {}({sig}) -> {} {{\n{prologue}{body_text}}}\n\n",
                    self.name,
                    ret.rust()
                )
            }
            FnKind::Writer {
                target,
                params,
                value,
            } => {
                let (sig, prologue) = render_params(params);
                let comma = if sig.is_empty() { "" } else { ", " };
                format!(
                    "fn {}(diff_target: &mut {}{comma}{sig}) {{\n{prologue}    let diff_cur: {} = diff_target.clone();\n    *diff_target = {};\n}}\n\n",
                    self.name,
                    target.rust(),
                    target.rust(),
                    value.render()
                )
            }
            FnKind::GenericPick => format!(
                "fn {}<T: Clone + std::fmt::Debug>(a: T, b: T, first: bool) -> T {{\n    if first {{ a }} else {{ b }}\n}}\n\n",
                self.name
            ),
            FnKind::Apply { ty } => format!(
                "fn {}<F: FnMut({}) -> {}>(f: &mut F, x: {}) -> {} {{\n    f(x)\n}}\n\n",
                self.name,
                ty.rust(),
                ty.rust(),
                ty.rust(),
                ty.rust()
            ),
            FnKind::Factory { ty, op } => format!(
                "fn {}(diff_n: {}) -> impl Fn({}) -> {} {{\n    move |diff_x: {}| (diff_x {} diff_n)\n}}\n\n",
                self.name,
                ty.rust(),
                ty.rust(),
                ty.rust(),
                ty.rust(),
                op.token()
            ),
        }
    }

    /// Every expression the definition holds.
    pub fn exprs(&self) -> Vec<&Expr> {
        match &self.kind {
            FnKind::Plain { body, .. } => vec![body],
            FnKind::Writer { value, .. } => vec![value],
            _ => Vec::new(),
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match &mut self.kind {
            FnKind::Plain { body, .. } => vec![body],
            FnKind::Writer { value, .. } => vec![value],
            _ => Vec::new(),
        }
    }

    pub fn feature(&self) -> &'static str {
        match &self.kind {
            FnKind::Plain { .. } => "lang-fn-def",
            FnKind::Writer { .. } => "lang-fn-writer",
            FnKind::GenericPick => "lang-fn-generic",
            FnKind::Apply { .. } => "lang-fn-apply",
            FnKind::Factory { .. } => "lang-fn-factory",
        }
    }
}

/// The signature text and the prologue that clones borrowed parameters
/// into the owned locals the body is typed against.
fn render_params(params: &[Param]) -> (String, String) {
    let mut sig = Vec::new();
    let mut prologue = String::new();
    for param in params {
        match param.mode {
            ParamMode::Owned => sig.push(format!("{}: {}", param.name, param.ty.rust())),
            ParamMode::Ref => {
                sig.push(format!("{}_ref: &{}", param.name, param.ty.rust()));
                prologue.push_str(&format!(
                    "    let {}: {} = {}_ref.clone();\n",
                    param.name,
                    param.ty.rust(),
                    param.name
                ));
            }
        }
    }
    (sig.join(", "), prologue)
}

/// The bindings a statement list writes to, for `mut` on its own lets.
pub fn block_mutable(stmts: &[Stmt]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for stmt in stmts {
        stmt.assigned(&mut out);
    }
    out
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    pub expr: Expr,
}

impl ConstDef {
    pub fn render(&self) -> String {
        format!(
            "const {}: {} = {};\n\n",
            self.name,
            self.ty.rust(),
            self.expr.render()
        )
    }
}

/// One generated program body, plus the items its statements use.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub statements: Vec<Stmt>,
    #[serde(default)]
    pub fns: Vec<FnDef>,
    #[serde(default)]
    pub consts: Vec<ConstDef>,
    #[serde(default)]
    pub types: Vec<UserDef>,
    /// Builtin types the block implements `DiffDescribe` for.
    #[serde(default)]
    pub describes: Vec<Ty>,
}

impl Block {
    pub fn mutable_names(&self) -> BTreeSet<String> {
        block_mutable(&self.statements)
    }

    pub fn render(&self) -> String {
        let mutable = self.mutable_names();
        let mut out = String::new();
        for stmt in &self.statements {
            out.push_str(&stmt.render(&mutable, 1));
        }
        out
    }

    /// The items rendered above `fn main`: types, consts, and helper
    /// functions. The describe impls on builtin types are rendered by the
    /// program, once across every block, because two blocks may name the
    /// same builtin type.
    pub fn render_items(&self) -> String {
        let mut out = String::new();
        for def in &self.types {
            out.push_str(&def.render());
        }
        for def in &self.consts {
            out.push_str(&def.render());
        }
        for def in &self.fns {
            out.push_str(&def.render());
        }
        out
    }

    /// Whether the program needs the `DiffDescribe` trait declared.
    pub fn uses_describe(&self) -> bool {
        !self.describes.is_empty() || self.types.iter().any(|def| def.shape.describe)
    }

    fn all_exprs(&self) -> Vec<&Expr> {
        let mut out: Vec<&Expr> = self.statements.iter().flat_map(Stmt::exprs).collect();
        out.extend(self.fns.iter().flat_map(FnDef::exprs));
        out.extend(self.types.iter().flat_map(|def| {
            def.methods
                .iter()
                .map(|method| &method.body)
                .chain(def.froms.iter().flat_map(|from| from.rest.iter()))
        }));
        out
    }

    pub fn helpers(&self) -> BTreeSet<Helper> {
        let mut out = BTreeSet::new();
        for expr in self.all_exprs() {
            expr.helpers(&mut out);
        }
        out
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        for stmt in &self.statements {
            stmt.features(out);
        }
        for def in &self.fns {
            out.insert(def.feature());
            for expr in def.exprs() {
                expr.features(out);
            }
        }
        for def in &self.consts {
            out.insert("lang-const-def");
            out.insert(def.ty.feature());
        }
        for def in &self.types {
            out.insert(if def.shape.is_enum() {
                "lang-enum-def"
            } else {
                "lang-struct-def"
            });
            if def.display.is_some() {
                out.insert("lang-display-impl");
            }
            if !def.froms.is_empty() {
                out.insert("lang-from-impl");
            }
            if !def.methods.is_empty() {
                out.insert("lang-method-def");
            }
            if def.shape.describe {
                out.insert("lang-trait-impl");
            }
            for method in &def.methods {
                method.body.features(out);
            }
        }
        if !self.describes.is_empty() {
            out.insert("lang-trait-impl-builtin");
        }
    }

    pub fn shape(&self, out: &mut String) {
        out.push_str("lang[");
        for stmt in &self.statements {
            stmt.shape(out);
        }
        for def in &self.fns {
            out.push_str("fn:");
            out.push_str(def.feature());
            out.push(',');
        }
        for def in &self.types {
            out.push_str("ty:");
            out.push_str(&def.shape.name);
            out.push(',');
        }
        out.push(']');
    }

    /// Every literal is laundered as soon as the block contains anything
    /// that can abort, so an overflow stays a runtime panic that the harness
    /// can compare instead of a compile error that wastes the case.
    pub fn seal(&mut self) {
        let fallible = self.statements.iter().any(Stmt::has_fallible_op)
            || self
                .fns
                .iter()
                .flat_map(FnDef::exprs)
                .any(Expr::has_fallible_op)
            || self
                .types
                .iter()
                .flat_map(|def| def.methods.iter())
                .any(|method| method.body.has_fallible_op());
        if !fallible {
            return;
        }
        for stmt in &mut self.statements {
            stmt.make_opaque();
        }
        for def in &mut self.fns {
            for expr in def.exprs_mut() {
                expr.make_opaque();
            }
        }
        for def in &mut self.types {
            for method in &mut def.methods {
                method.body.make_opaque();
            }
        }
    }

    /// Drop helper functions, consts, and types nothing refers to anymore,
    /// so a shrunk program never carries an orphan definition.
    fn retain_used(&mut self) {
        let statements_text: String = self
            .statements
            .iter()
            .map(|s| s.render(&BTreeSet::new(), 0))
            .collect();
        let fn_text: String = self.fns.iter().map(FnDef::render).collect();
        let called = |name: &str| {
            self.statements.iter().any(|stmt| {
                stmt.exprs().iter().any(|expr| expr.calls_fn(name))
                    || matches!(stmt, Stmt::CallMut { fn_name, .. } if fn_name == name)
                    || matches!(
                        stmt,
                        Stmt::LetClosure {
                            source: ClosureSource::Factory { fn_name, .. },
                            ..
                        } if fn_name == name
                    )
            }) || self
                .fns
                .iter()
                .flat_map(FnDef::exprs)
                .any(|expr| expr.calls_fn(name))
        };
        let kept_fns: Vec<FnDef> = self
            .fns
            .iter()
            .filter(|def| called(&def.name))
            .cloned()
            .collect();
        self.fns = kept_fns;
        let uses = |name: &str| statements_text.contains(name) || fn_text.contains(name);
        self.consts.retain(|def| uses(&def.name));
        // A type can be named only by another type, so drop until stable.
        loop {
            let types_text: String = self.types.iter().map(UserDef::render).collect();
            let before = self.types.len();
            self.types.retain(|def| {
                uses(&def.shape.name)
                    || types_text.matches(&def.shape.name).count() > occurrences_in_own(def)
            });
            if self.types.len() == before {
                break;
            }
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        for index in 0..self.statements.len() {
            candidates.push(self.without(index));
            for stmt in self.statements[index].shrinks() {
                let mut candidate = self.clone();
                candidate.statements[index] = stmt;
                candidate.seal();
                candidates.push(candidate);
            }
        }
        for index in 0..self.fns.len() {
            let count = self.fns[index].exprs().len();
            for slot in 0..count {
                for shrunk in self.fns[index].exprs()[slot].shrinks() {
                    let mut candidate = self.clone();
                    if let Some(target) = candidate.fns[index].exprs_mut().into_iter().nth(slot) {
                        *target = shrunk;
                    }
                    candidate.seal();
                    candidates.push(candidate);
                }
            }
        }
        candidates
    }

    /// Drop one statement, and everything that depended on it, so the result
    /// still compiles.
    fn without(&self, index: usize) -> Self {
        let mut dropped: BTreeSet<String> = self.statements[index].declared().into_iter().collect();
        let mut statements = Vec::new();
        for (position, stmt) in self.statements.iter().enumerate() {
            if position == index {
                continue;
            }
            if stmt.uses_any(&dropped) || stmt.writes_any(&dropped) {
                dropped.extend(stmt.declared());
                continue;
            }
            statements.push(stmt.clone());
        }
        let mut candidate = Self {
            statements,
            fns: self.fns.clone(),
            consts: self.consts.clone(),
            types: self.types.clone(),
            describes: self.describes.clone(),
        };
        candidate.retain_used();
        candidate.seal();
        candidate
    }
}

/// How often a type's own declaration mentions its name: the declaration
/// line and each impl header, which must not count as a use by another type.
fn occurrences_in_own(def: &UserDef) -> usize {
    def.render().matches(&def.shape.name).count()
}
