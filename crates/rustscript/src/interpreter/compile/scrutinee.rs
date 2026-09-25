//! How a pattern owns its scrutinee, whether its bindings take parts out of a value of its
//! own, and where the rest of that value drops once they did.

use syn::Expr;

use super::super::bytecode::Reg;
use super::place::single_path_name;
use super::walks::unparen;
use super::{Compiler, NameLoc};

/// Where a scrutinee's shell drops after its bound parts moved out, see `shell_home`.
#[derive(Clone, Copy)]
pub(super) enum ShellHome {
    /// the bindings' own scope, after the bindings
    Scope,
    /// the scope of the moved local, at the slot after it
    Local {
        scope: usize,
        at: usize,
    },
    None,
}

impl Compiler<'_> {
    /// Whether a pattern over the expression takes its bindings out of a value of its own. A
    /// borrow parameter, `self` in a `&self` method, a reference or an accessor only lends its
    /// parts, so the bindings must not drop at the arm's end.
    pub(super) fn scrutinee_owned(&mut self, expr: &Expr) -> bool {
        match expr {
            Expr::Paren(p) => self.scrutinee_owned(&p.expr),
            Expr::Group(g) => self.scrutinee_owned(&g.expr),
            Expr::Reference(_) => false,
            Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = p.path.segments[0].ident.to_string();
                match self.resolve(&name) {
                    NameLoc::Local(reg) => {
                        let f = self.cur();
                        !f.shares_only(reg) && !f.drop_exempt.contains(&reg)
                    }
                    _ => true,
                }
            }
            other => self.init_owned(other),
        }
    }

    /// A scrutinee a pattern may take parts of. `(make()).f` moves its field out of the
    /// temporary, see `compile_owned_into`, so the field is a fresh value of its own.
    pub(super) fn pattern_scrutinee_owned(&mut self, expr: &Expr) -> bool {
        self.scrutinee_owned(expr) || self.fresh_field(expr)
    }

    /// A non copy field of a fresh value, taken out of the temporary that holds the rest.
    fn fresh_field(&mut self, expr: &Expr) -> bool {
        match unparen(expr) {
            Expr::Field(f) => {
                self.ctx.has_drop && self.moves_out(unparen(expr)) && self.fresh_scrutinee(&f.base)
            }
            _ => false,
        }
    }

    /// An owned scrutinee nobody else holds, a call or a constructor. A local read by `Own` may
    /// be a copy when it lives on, so only a fresh value drops on the path that binds nothing.
    pub(super) fn fresh_scrutinee(&mut self, expr: &Expr) -> bool {
        match expr {
            Expr::Paren(p) => self.fresh_scrutinee(&p.expr),
            Expr::Group(g) => self.fresh_scrutinee(&g.expr),
            Expr::Path(_)
            | Expr::Field(_)
            | Expr::Index(_)
            | Expr::Reference(_)
            | Expr::Unary(_) => false,
            other => self.scrutinee_owned(other),
        }
    }

    /// Where the shell of an owned scrutinee drops once `TakeBinds` moved its bound parts
    /// out. A fresh value drops with the bindings' scope. A local read by move is a partial
    /// move, its rest drops where the local was declared, right after it in reverse order.
    /// Only a program with a `Drop` impl can observe either.
    pub(super) fn shell_home(&mut self, expr: &Expr) -> ShellHome {
        if !self.ctx.has_drop {
            return ShellHome::None;
        }
        if self.fresh_scrutinee(expr) || self.fresh_field(expr) {
            return ShellHome::Scope;
        }
        let Some(name) = single_path_name(unparen(expr)) else {
            return ShellHome::None;
        };
        let name = self.unalias(&name);
        let NameLoc::Local(reg) = self.resolve(&name) else {
            return ShellHome::None;
        };
        let f = self.cur();
        let Some(scope) = f.scope_order.iter().rposition(|regs| regs.contains(&reg)) else {
            return ShellHome::None;
        };
        let at = f.scope_order[scope]
            .iter()
            .position(|r| *r == reg)
            .map_or(0, |at| at + 1);
        ShellHome::Local { scope, at }
    }

    /// Holds the shell in its home scope so the scope's drops end it, see `shell_home`.
    pub(super) fn hold_shell(&mut self, shell: Reg, home: ShellHome) {
        let f = self.cur();
        match home {
            ShellHome::Scope => f
                .scope_order
                .last_mut()
                .expect("a scope is always open")
                .push(shell),
            ShellHome::Local { scope, at } => f.scope_order[scope].insert(at, shell),
            ShellHome::None => {}
        }
    }
}
