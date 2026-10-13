//! What a use after the `let` says about a local. `let mut v = Vec::new()` is a vec of nothing
//! known until `v.push(x)` names the item. A value that stays empty at run time has no element
//! to show its type, so this table is all that `concat`, `unwrap_or_default` or a trait impl on
//! `Vec<f64>` can go by.

use syn::Expr;

use super::{Infer, Ty};

impl Infer<'_, '_> {
    /// A method that puts a value into its receiver names the element type of a local that has
    /// none yet.
    pub(super) fn refine_receiver(&mut self, m: &syn::ExprMethodCall, recv: &Ty) {
        if !recv.has_unknown() {
            return;
        }
        let Some(name) = local_name(&m.receiver) else {
            return;
        };
        let args: Vec<&Expr> = m.args.iter().collect();
        if args.iter().any(|arg| self.is_open(arg)) {
            return;
        }
        let arg = |i: usize| {
            args.get(i)
                .map_or(Ty::Unknown, |arg| self.typed_so_far(arg))
        };
        let Some(learned) = learned_from(recv, &m.method.to_string(), arg) else {
            return;
        };
        self.refine_local(&name, &learned);
    }

    /// `v = make()` names the type of a local declared with none, `let mut found = None`.
    pub(super) fn refine_assigned(&mut self, a: &syn::ExprAssign, value: &Ty) {
        if self.is_open(&a.right) {
            return;
        }
        if let Some(name) = local_name(&a.left) {
            self.refine_local(&name, value);
        }
    }

    fn typed_so_far(&self, e: &Expr) -> Ty {
        self.types
            .get(&std::ptr::from_ref(e))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    /// An `into` no context typed is recorded as its receiver type, which says nothing of the
    /// type it lands in.
    fn is_open(&self, e: &Expr) -> bool {
        match e {
            Expr::Paren(p) => self.is_open(&p.expr),
            Expr::Group(g) => self.is_open(&g.expr),
            Expr::Reference(r) => self.is_open(&r.expr),
            Expr::MethodCall(m) => self.unresolved.contains(&std::ptr::from_ref(m).cast()),
            _ => false,
        }
    }
}

/// The type of the receiver as the arguments of a storing method show it.
fn learned_from(recv: &Ty, method: &str, arg: impl Fn(usize) -> Ty) -> Option<Ty> {
    let boxed = |i: usize| Box::new(arg(i));
    Some(match (recv, method) {
        (Ty::Vec(_), "push" | "push_back" | "push_front" | "fill") => Ty::Vec(boxed(0)),
        (Ty::Vec(_), "insert" | "resize") => Ty::Vec(boxed(1)),
        (Ty::Vec(_), "extend" | "extend_from_slice" | "append") => Ty::vec(arg(0).item()),
        (Ty::Set(_, sorted), "insert") => Ty::Set(boxed(0), *sorted),
        (Ty::Set(_, sorted), "extend" | "append") => Ty::Set(Box::new(arg(0).item()), *sorted),
        (Ty::Map(_, _, sorted), "insert") => Ty::Map(boxed(0), boxed(1), *sorted),
        (Ty::Map(_, _, sorted), "entry") => Ty::Map(boxed(0), Box::new(Ty::Unknown), *sorted),
        (Ty::Map(_, _, sorted), "extend" | "append") => match arg(0).item() {
            Ty::Tuple(pair) if pair.len() == 2 => Ty::Map(
                Box::new(pair[0].clone()),
                Box::new(pair[1].clone()),
                *sorted,
            ),
            _ => return None,
        },
        _ => return None,
    })
}

/// The local a place expression names, through parens, a borrow and a deref.
fn local_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Paren(p) => local_name(&p.expr),
        Expr::Group(g) => local_name(&g.expr),
        Expr::Reference(r) => local_name(&r.expr),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => local_name(&u.expr),
        Expr::Path(p) if p.qself.is_none() => p.path.get_ident().map(ToString::to_string),
        _ => None,
    }
}
