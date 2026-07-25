//! Statements and blocks. A generated block is a real `fn main` body: typed
//! bindings, reassignment, nested control flow, and a labeled print per
//! observation so a mismatch names the line that produced it.

use std::collections::BTreeSet;

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
}

impl Stmt {
    /// Names this statement writes to, including inside nested blocks, so the
    /// renderer knows which bindings need `mut`.
    pub fn assigned(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Assign { name, .. } => {
                out.insert(name.clone());
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
        }
    }

    /// Whether this statement assigns to a name it does not own, which makes
    /// it invalid once that binding is dropped by the reducer.
    pub fn writes_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Assign { name, .. } => names.contains(name),
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
            Self::Print { label, expr } => {
                format!("{pad}println!(\"{label}: {{:?}}\", {});\n", expr.render())
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
        }
    }
}

/// One generated program body.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub statements: Vec<Stmt>,
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

    pub fn helpers(&self) -> BTreeSet<Helper> {
        let mut out = BTreeSet::new();
        for stmt in &self.statements {
            stmt.helpers(&mut out);
        }
        out
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        for stmt in &self.statements {
            stmt.features(out);
        }
    }

    pub fn shape(&self, out: &mut String) {
        out.push_str("lang[");
        for stmt in &self.statements {
            stmt.shape(out);
        }
        out.push(']');
    }

    /// Every integer literal is laundered as soon as the block contains
    /// anything that can abort, so an overflow stays a runtime panic that the
    /// harness can compare instead of a compile error that wastes the case.
    pub fn seal(&mut self) {
        if self.statements.iter().any(Stmt::has_fallible_op) {
            self.statements.iter_mut().for_each(Stmt::make_opaque);
        }
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
        let mut candidate = Self { statements };
        candidate.seal();
        Some(candidate)
    }
}
