//! Macro typing. A body is parsed once here and the compiler lowers the same nodes.

use std::rc::Rc;

use syn::Expr;

use super::super::support::{parse_exprs, parse_matches, parse_vec_repeat};
use super::{Infer, MacroBody, Ty};
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    pub(super) fn macro_ty(&mut self, mac: &syn::Macro, expected: &Ty) -> Ty {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        match name.as_str() {
            "vec" => {
                if let Ok((value, count)) = mac.parse_body_with(parse_vec_repeat) {
                    let item = self.expr(&value, &expected.item());
                    self.expr(&count, &Ty::usize());
                    let body = Rc::new(MacroBody::Repeat(Box::new((value, count))));
                    self.macros.insert(std::ptr::from_ref(mac), body);
                    return Ty::vec(item);
                }
                let Ok(exprs) = parse_exprs(mac) else {
                    return Ty::vec(Ty::Unknown);
                };
                let mut item = expected.item();
                for e in &exprs {
                    let got = self.expr(e, &item);
                    item = self.vars.meet(&item, &got);
                }
                self.macros
                    .insert(std::ptr::from_ref(mac), Rc::new(MacroBody::Exprs(exprs)));
                Ty::vec(item)
            }
            "matches" => {
                let Ok((scrutinee, pat, guard)) = parse_matches(mac) else {
                    return Ty::Bool;
                };
                let ty = self.expr(&scrutinee, &Ty::Unknown);
                self.push();
                self.bind_pat(&pat, &ty);
                if let Some(guard) = &guard {
                    self.expr(guard, &Ty::Bool);
                }
                self.pop();
                self.macros.insert(
                    std::ptr::from_ref(mac),
                    Rc::new(MacroBody::Matches(Box::new((scrutinee, pat, guard)))),
                );
                Ty::Bool
            }
            "format" | "println" | "print" | "eprintln" | "eprint" | "panic" | "anyhow"
            | "bail" | "unreachable" | "todo" | "unimplemented" | "write" | "writeln"
            | "assert" | "assert_eq" | "assert_ne" | "debug_assert" | "debug_assert_eq"
            | "debug_assert_ne" | "ensure" | "dbg" | "join" => {
                let Ok(exprs) = parse_exprs(mac) else {
                    return Ty::Unknown;
                };
                let first = self.fmt_macro_ty(&name, &exprs, expected);
                self.macros
                    .insert(std::ptr::from_ref(mac), Rc::new(MacroBody::Exprs(exprs)));
                first
            }
            "json" => Ty::Json,
            "include_str" | "env" | "concat" | "stringify" | "file" => Ty::Str,
            "line" | "column" => Ty::Int(IntWidth::U32),
            _ => Ty::Unknown,
        }
    }

    /// The format family, the asserts and the few other macros with a plain argument list.
    fn fmt_macro_ty(&mut self, name: &str, exprs: &[Expr], expected: &Ty) -> Ty {
        match name {
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                let mut pair = exprs.iter();
                let left = pair.next().map(|e| self.expr(e, &Ty::Unknown));
                if let (Some(left), Some(right)) = (left, pair.next()) {
                    let right = self.expr(right, &left);
                    self.vars.unify(&left, &right);
                }
                for e in pair {
                    self.format_arg(e);
                }
                Ty::Unit
            }
            "assert" | "debug_assert" | "ensure" => {
                let mut it = exprs.iter();
                if let Some(c) = it.next() {
                    self.expr(c, &Ty::Bool);
                }
                for e in it {
                    self.format_arg(e);
                }
                Ty::Unit
            }
            "dbg" => exprs.first().map_or(Ty::Unit, |e| self.expr(e, expected)),
            "join" => Ty::Tuple(
                exprs
                    .iter()
                    .map(|e| {
                        let handle = self.expr(e, &Ty::Unknown);
                        match handle {
                            Ty::Named(n, args) if &*n == "JoinHandle" => Ty::result(
                                args.into_iter().next().unwrap_or(Ty::Unknown),
                                Ty::named("JoinError"),
                            ),
                            other => other,
                        }
                    })
                    .collect(),
            ),
            "write" | "writeln" => {
                let mut it = exprs.iter();
                if let Some(w) = it.next() {
                    self.expr(w, &Ty::Unknown);
                }
                for e in it.skip(1) {
                    self.format_arg(e);
                }
                Ty::result(Ty::Unit, Ty::named("io::Error"))
            }
            _ => {
                for e in exprs.iter().skip(1) {
                    self.format_arg(e);
                }
                match name {
                    "format" => Ty::Str,
                    "anyhow" => Ty::named("anyhow::Error"),
                    "println" | "print" | "eprintln" | "eprint" => Ty::Unit,
                    _ => Ty::Unknown,
                }
            }
        }
    }

    /// `name = value` and plain arguments, a bare literal ends as `i32` like in `rustc`.
    fn format_arg(&mut self, e: &Expr) {
        match e {
            Expr::Assign(a) if matches!(&*a.left, Expr::Path(p) if p.path.get_ident().is_some()) => {
                self.expr(&a.right, &Ty::Unknown);
            }
            other => {
                self.expr(other, &Ty::Unknown);
            }
        }
    }
}
