//! `RefCell` guard release. A guard is a value, so the register that holds it keeps the borrow
//! alive. Real Rust ends a temporary with its statement and a binding with its scope, so the
//! compiler emits the same releases, and the VM clears a guard frame on return.

use syn::Expr;

use crate::interpreter::bytecode::{BuiltinId, Op, Reg};

use super::{Compiler, idx16};

impl Compiler<'_> {
    /// A `RefCell` guard temporary ends with its statement like a Rust temporary, or the next
    /// borrow would find it still live. Temporaries since `mark` are released, except `keep` and
    /// the ones a `let` just named, those become guard bindings for the scope end. Returns the
    /// drop list so a second path can release the same registers.
    pub(super) fn release_guard_temps(&mut self, mark: usize, keep: Option<Reg>) -> Option<u16> {
        let f = self.cur();
        if f.guard_temps.len() <= mark {
            return None;
        }
        let temps = f.guard_temps.split_off(mark);
        let mut regs = Vec::new();
        for reg in temps {
            if Some(reg) == keep {
                continue;
            }
            if f.scopes
                .iter()
                .any(|scope| scope.values().any(|&r| r == reg))
            {
                f.guard_regs.insert(reg);
            } else {
                regs.push(reg);
            }
        }
        if regs.is_empty() {
            return None;
        }
        f.drop_lists.push(regs.into());
        let list = idx16(f.drop_lists.len() - 1);
        self.emit(Op::DropScope { list });
        Some(list)
    }

    /// A `let` whose value came out of a `borrow` holds a guard until its scope ends.
    pub(super) fn note_guard_binding(&mut self, init: &Expr, before: usize) {
        if !self.init_holds_guard(init) {
            return;
        }
        let f = self.cur();
        let bound: Vec<Reg> = f
            .scope_order
            .last()
            .map_or(Vec::new(), |regs| regs[before..].to_vec());
        f.guard_regs.extend(bound);
        f.has_guards = true;
    }

    pub(super) fn init_holds_guard(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Paren(p) => self.init_holds_guard(&p.expr),
            Expr::Group(g) => self.init_holds_guard(&g.expr),
            Expr::Try(t) => self.init_holds_guard(&t.expr),
            Expr::MethodCall(m) => {
                BuiltinId::resolve(&m.method.to_string()).is_borrow()
                    || (matches!(
                        m.method.to_string().as_str(),
                        "unwrap" | "expect" | "unwrap_or_else" | "ok" | "expect_err"
                    ) && self.init_holds_guard(&m.receiver))
            }
            Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = p.path.segments[0].ident.to_string();
                let f = self.frames.last().expect("a frame");
                f.scopes
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name))
                    .is_some_and(|reg| f.guard_regs.contains(reg))
            }
            _ => false,
        }
    }
}
