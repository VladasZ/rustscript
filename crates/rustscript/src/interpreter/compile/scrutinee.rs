//! How a pattern owns its scrutinee, whether its bindings take parts out of a value of its
//! own, and where the rest of that value drops once they did.

use std::ptr::from_ref;

use syn::{Expr, Pat};

use super::super::bytecode::Reg;
use super::infer::Ty;
use super::place::{copies, single_path_name};
use super::support::{chain_lends, returns_borrow};
use super::walks::{block_tail, first_arm, then_tail, unparen};
use super::{Compiler, FnState, NameLoc};

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

    /// Whether the expression hands out a reference the frame already holds, a reference
    /// local, a borrow parameter or a binding that only holds a borrowed handle by its name,
    /// also as the value of an `if`, a `match` or a block, `if c { r } else { q }`. Nothing
    /// made here owns the storage behind it.
    pub(super) fn forwards_ref(&self, expr: &Expr) -> bool {
        match unparen(expr) {
            Expr::If(i) => then_tail(i).is_some_and(|e| self.forwards_ref(e)),
            Expr::Match(m) => first_arm(m).is_some_and(|e| self.forwards_ref(e)),
            Expr::Block(b) if b.label.is_none() => {
                block_tail(&b.block).is_some_and(|e| self.forwards_ref(e))
            }
            other => self
                .named_local(other)
                .is_some_and(|(f, reg)| f.shares_only(reg) || f.drop_exempt.contains(&reg)),
        }
    }

    /// Whether the name is a binding that holds a reference, a borrow parameter, a reference
    /// local or a value with references inside.
    pub(super) fn names_ref(&self, expr: &Expr) -> bool {
        self.named_local(expr)
            .is_some_and(|(f, reg)| f.shares_only(reg) || f.ref_holders.contains(&reg))
    }

    /// Whether the name is a borrow parameter that still sits in its own register.
    pub(super) fn names_ref_param(&self, expr: &Expr) -> bool {
        self.named_local(expr).is_some_and(|(f, reg)| {
            f.borrow_params.contains(&reg) && !f.mutable_locals.contains(&reg)
        })
    }

    /// Whether the name is a binding that holds a reference to an item of a collection, see
    /// `FnState::lent_binds`, and the item has parts a holder could drop.
    pub(super) fn names_lent_item(&self, expr: &Expr) -> bool {
        self.named_local(expr)
            .is_some_and(|(f, reg)| f.lent_binds.contains(&reg))
            && self.has_parts_to_drop(expr)
    }

    /// The bindings a `for` loop made since `before` over a source that is not its own. The
    /// loop end drops none of them. Over a borrowed collection, or over a chain that only
    /// lends its items, they are references to those items.
    pub(super) fn lend_loop_binds(&mut self, source: &Expr, borrowed: bool, before: usize) {
        let lends = self.ctx.has_drop
            && (borrowed
                || matches!(unparen(source), Expr::Reference(r) if r.mutability.is_none())
                || chain_lends(source));
        let f = self.cur();
        let bound: Vec<Reg> = f
            .scope_order
            .last()
            .map_or(Vec::new(), |regs| regs[before..].to_vec());
        if lends {
            let lent: Vec<Reg> = bound
                .iter()
                .copied()
                .filter(|reg| !f.own_copies.contains(reg))
                .collect();
            f.lent_binds.extend(lent);
        }
        f.drop_exempt.extend(bound);
    }

    /// A call of a script function or method that returns a reference, or a value with a
    /// reference inside, hands out storage someone else owns, so nothing drops it here.
    pub(super) fn lends_result(&self, expr: &Expr) -> bool {
        match unparen(expr) {
            Expr::Call(call) => self.call_lends(call),
            Expr::MethodCall(m) => {
                let name = m.method.to_string();
                let mut sigs = self
                    .ctx
                    .impl_sigs
                    .iter()
                    .filter(|((_, method), _)| *method == name)
                    .peekable();
                sigs.peek().is_some() && sigs.all(|(_, sig)| returns_borrow(sig))
            }
            _ => false,
        }
    }

    /// What a method call does with references to items, noted right before its arguments
    /// compile. An iterator method over a chain that only lends its items hands its closure
    /// a reference, so that parameter is marked for `lent_closure_param`. And a method that
    /// takes its argument by value keeps it, hands it back or throws it away, `push`,
    /// `insert`, `unwrap_or` and `then_some`. So a borrow, `held.push(&x)`, goes in behind a
    /// reference value that drops nothing, like an element of a tuple literal, see
    /// `compile_elems`.
    pub(super) fn note_lent_refs(&mut self, m: &syn::ExprMethodCall, method: &str) {
        if !self.ctx.has_drop {
            return;
        }
        let param = match method {
            "map" | "filter_map" | "flat_map" | "map_while" | "for_each" | "find_map" | "any"
            | "all" => Some(0),
            "fold" | "scan" => Some(1),
            _ => None,
        };
        if let Some(index) = param
            && chain_lends(&m.receiver)
            && let Some(Expr::Closure(c)) = m.args.last().map(unparen)
        {
            self.cur().next_call.lent_closure = Some((from_ref(c).addr(), index));
        }
        let receiver = self.types.of(&m.receiver);
        let collection = matches!(receiver, Ty::Vec(_) | Ty::Set(..) | Ty::Map(..));
        let option = matches!(receiver, Ty::Option(_));
        let by_value = match method {
            "push" | "push_back" | "push_front" | "entry" => collection,
            "insert" => collection || option,
            "unwrap_or" | "or" | "get_or_insert" | "replace" | "map_or" => option,
            "then_some" => receiver == Ty::Bool,
            _ => false,
        };
        if by_value {
            let lent: Vec<bool> = m.args.iter().map(|arg| self.lends_storage(arg)).collect();
            if lent.contains(&true) {
                self.cur().next_call.value_refs = lent;
            }
        }
    }

    /// The index of the parameter of this closure that gets a reference to an item, when
    /// the method call around it said so, see `note_lent_refs`.
    pub(super) fn lent_closure_param(&mut self, c: &syn::ExprClosure) -> Option<usize> {
        let (addr, index) = self.cur().next_call.lent_closure?;
        if addr != from_ref(c).addr() {
            return None;
        }
        self.cur().next_call.lent_closure = None;
        Some(index)
    }

    /// The bindings the open scope made since `before` are a closure parameter that holds a
    /// reference to an item. The closure neither owns nor drops them.
    pub(super) fn note_lent_param(&mut self, before: usize) {
        let f = self.cur();
        let bound: Vec<Reg> = f
            .scope_order
            .last()
            .map_or(Vec::new(), |regs| regs[before..].to_vec());
        f.lent_binds.extend(bound.iter().copied());
        f.drop_exempt.extend(bound);
    }

    /// The local a bare name reads and its frame, `None` for an alias and anything else.
    fn named_local(&self, expr: &Expr) -> Option<(&FnState, Reg)> {
        let name = single_path_name(unparen(expr))?;
        let f = self.frames.last()?;
        if f.aliases.contains_key(&name) {
            return None;
        }
        f.local_reg(&name).map(|reg| (f, reg))
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

impl Compiler<'_> {
    /// Whether a match arm moves a part out of its scrutinee. A by value binding of a `Copy`
    /// part copies it, and a type inference left open counts as a move.
    pub(super) fn pattern_moves(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Ident(id) if super::pattern::is_unit_variant_ident(id) => false,
            Pat::Ident(id) => {
                (id.by_ref.is_none() && !copies(&self.types.of_bind(id)))
                    || id
                        .subpat
                        .as_ref()
                        .is_some_and(|sub| self.pattern_moves(&sub.1))
            }
            Pat::Tuple(t) => t.elems.iter().any(|p| self.pattern_moves(p)),
            Pat::TupleStruct(ts) => ts.elems.iter().any(|p| self.pattern_moves(p)),
            Pat::Slice(s) => s.elems.iter().any(|p| self.pattern_moves(p)),
            Pat::Struct(s) => s.fields.iter().any(|f| self.pattern_moves(&f.pat)),
            Pat::Paren(p) => self.pattern_moves(&p.pat),
            Pat::Type(t) => self.pattern_moves(&t.pat),
            Pat::Or(o) => o.cases.iter().any(|p| self.pattern_moves(p)),
            _ => false,
        }
    }

    /// The local an owned scrutinee was read out of, see `ShellHome::Local`.
    pub(super) fn scrutinee_local(&mut self, expr: &Expr) -> Option<Reg> {
        let name = single_path_name(unparen(expr))?;
        let name = self.unalias(&name);
        match self.resolve(&name) {
            NameLoc::Local(reg) => Some(reg),
            _ => None,
        }
    }
}
