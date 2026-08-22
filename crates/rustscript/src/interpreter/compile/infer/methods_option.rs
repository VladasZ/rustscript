//! `Option`, `Result` and the iterator adapters, the tables that carry a payload type through.

use syn::Expr;

use super::{Infer, Ty};

impl Infer<'_, '_> {
    pub(super) fn option_method(
        &mut self,
        recv: &Ty,
        payload: &Ty,
        name: &str,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        match name {
            "unwrap" | "expect" | "unwrap_unchecked" => {
                self.walk_all(args);
                payload.clone()
            }
            "unwrap_or" => {
                let got = self.arg_ty(args, 0, payload);
                self.vars.meet(payload, &got)
            }
            "unwrap_or_else" => {
                let got = self.closure_ret(args, 0, Vec::new());
                self.vars.meet(payload, &got)
            }
            "unwrap_or_default" => self.vars.meet(payload, expected),
            "is_some" | "is_none" => Ty::Bool,
            "is_some_and" | "is_none_or" => {
                self.closure_ret(args, 0, vec![payload.clone()]);
                Ty::Bool
            }
            "map" => {
                let want = match expected {
                    Ty::Option(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                let ret = self.closure_ret_expecting(args, 0, vec![payload.clone()], &want);
                Ty::option(ret)
            }
            "and_then" => {
                let ret = self.closure_ret_expecting(args, 0, vec![payload.clone()], expected);
                match ret {
                    Ty::Option(_) => ret,
                    other => Ty::option(other.payload()),
                }
            }
            "filter" | "inspect" => {
                self.closure_ret(args, 0, vec![payload.clone()]);
                recv.clone()
            }
            "take" | "as_ref" | "as_mut" | "as_deref" | "as_deref_mut" | "cloned" | "copied"
            | "clone" | "or" | "xor" | "or_else" | "replace" => {
                for arg in args {
                    self.expr(arg, recv);
                }
                recv.clone()
            }
            "and" => self.arg_ty(args, 0, expected),
            "ok_or" => {
                let err = self.arg_ty(args, 0, &Ty::Unknown);
                Ty::result(payload.clone(), err)
            }
            "ok_or_else" => {
                let err = self.closure_ret(args, 0, Vec::new());
                Ty::result(payload.clone(), err)
            }
            "map_or" => {
                let default = self.arg_ty(args, 0, expected);
                let got = self.closure_ret_expecting(args, 1, vec![payload.clone()], &default);
                self.vars.meet(&default, &got)
            }
            "map_or_else" => {
                let default = self.closure_ret(args, 0, Vec::new());
                let got = self.closure_ret_expecting(args, 1, vec![payload.clone()], &default);
                self.vars.meet(&default, &got)
            }
            "zip" => {
                let other = self.arg_ty(args, 0, &Ty::Unknown);
                Ty::option(Ty::Tuple(vec![payload.clone(), other.payload()]))
            }
            "iter" | "into_iter" | "iter_mut" => Ty::iter(payload.clone()),
            "get_or_insert_with" | "get_or_insert" | "insert" => {
                for arg in args {
                    self.expr(arg, &Ty::Closure(Vec::new(), Box::new(payload.clone())));
                }
                payload.clone()
            }
            "flatten" => payload.clone(),
            "unzip" => Ty::Unknown,
            _ => self.walk_all(args),
        }
    }

    pub(super) fn result_method(
        &mut self,
        recv: &Ty,
        ok: &Ty,
        err: &Ty,
        name: &str,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        match name {
            "unwrap" | "expect" => {
                self.walk_all(args);
                ok.clone()
            }
            "unwrap_err" | "expect_err" => {
                self.walk_all(args);
                err.clone()
            }
            "unwrap_or" => {
                let got = self.arg_ty(args, 0, ok);
                self.vars.meet(ok, &got)
            }
            "unwrap_or_else" => {
                let got = self.closure_ret(args, 0, vec![err.clone()]);
                self.vars.meet(ok, &got)
            }
            "unwrap_or_default" => self.vars.meet(ok, expected),
            "is_ok" | "is_err" => Ty::Bool,
            "is_ok_and" | "is_err_and" => {
                self.closure_ret(
                    args,
                    0,
                    vec![if name == "is_ok_and" {
                        ok.clone()
                    } else {
                        err.clone()
                    }],
                );
                Ty::Bool
            }
            "ok" => Ty::option(ok.clone()),
            "err" => Ty::option(err.clone()),
            "map" => {
                let want = match expected {
                    Ty::Result(t, _) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                let ret = self.closure_ret_expecting(args, 0, vec![ok.clone()], &want);
                Ty::result(ret, err.clone())
            }
            "map_err" => {
                let ret = self.closure_ret(args, 0, vec![err.clone()]);
                Ty::result(ok.clone(), ret)
            }
            "and_then" => {
                let ret = self.closure_ret_expecting(args, 0, vec![ok.clone()], expected);
                match ret {
                    Ty::Result(..) => ret,
                    other => Ty::result(other.payload(), err.clone()),
                }
            }
            "or_else" => {
                self.closure_ret(args, 0, vec![err.clone()]);
                recv.clone()
            }
            "context" | "with_context" => {
                self.walk_all(args);
                Ty::result(ok.clone(), Ty::named("anyhow::Error"))
            }
            "as_ref" | "as_mut" | "clone" | "cloned" | "copied" | "inspect" | "inspect_err" => {
                self.walk_all(args);
                recv.clone()
            }
            "map_or" => {
                let default = self.arg_ty(args, 0, expected);
                let got = self.closure_ret_expecting(args, 1, vec![ok.clone()], &default);
                self.vars.meet(&default, &got)
            }
            "iter" | "into_iter" => Ty::iter(ok.clone()),
            _ => self.walk_all(args),
        }
    }

    pub(super) fn closure_ret_expecting(
        &mut self,
        args: &[&Expr],
        i: usize,
        params: Vec<Ty>,
        want: &Ty,
    ) -> Ty {
        let closure = Ty::Closure(params, Box::new(want.clone()));
        match self.arg_ty(args, i, &closure) {
            Ty::Closure(_, ret) => *ret,
            _ => Ty::Unknown,
        }
    }

    pub(super) fn iter_method(
        &mut self,
        recv: &Ty,
        item: &Ty,
        name: &str,
        args: &[&Expr],
        turbofish: Option<Ty>,
        expected: &Ty,
    ) -> Ty {
        if let Some(ty) = self.iter_reduce(item, name, args, turbofish, expected) {
            return ty;
        }
        let same = Ty::iter(item.clone());
        match name {
            "iter" | "into_iter" | "iter_mut" | "rev" | "peekable" | "by_ref" | "cloned"
            | "copied" | "fuse" | "cycle" | "skip" | "take" | "step_by" => {
                for arg in args {
                    self.expr(arg, &Ty::usize());
                }
                same
            }
            "filter" | "skip_while" | "take_while" | "inspect" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                same
            }
            "map" => {
                let want = expected.item();
                let ret = self.closure_ret_expecting(args, 0, vec![item.clone()], &want);
                Ty::iter(ret)
            }
            "filter_map" | "map_while" => {
                let ret = self.closure_ret(args, 0, vec![item.clone()]);
                Ty::iter(ret.payload())
            }
            "flat_map" => {
                let ret = self.closure_ret(args, 0, vec![item.clone()]);
                Ty::iter(ret.item())
            }
            "flatten" => Ty::iter(item.item()),
            "enumerate" => Ty::iter(Ty::Tuple(vec![Ty::usize(), item.clone()])),
            "zip" => {
                let other = self.arg_ty(args, 0, &Ty::Unknown);
                Ty::iter(Ty::Tuple(vec![item.clone(), other.item()]))
            }
            "chain" => {
                self.arg_ty(args, 0, recv);
                same
            }
            "scan" => {
                let state = self.arg_ty(args, 0, &Ty::Unknown);
                let ret = self.closure_ret(args, 1, vec![state, item.clone()]);
                Ty::iter(ret.payload())
            }
            "windows" | "chunks" => {
                self.arg_ty(args, 0, &Ty::usize());
                Ty::iter(Ty::vec(item.clone()))
            }
            "last" | "next" | "nth" | "min" | "max" | "next_back" | "reduce" | "min_by_key"
            | "max_by_key" | "min_by" | "max_by" | "find" | "find_map" | "peek" | "nth_back" => {
                self.iter_pick(item, name, args)
            }
            "position" | "rposition" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                Ty::option(Ty::usize())
            }
            "any" | "all" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                Ty::Bool
            }
            "contains" => {
                self.arg_ty(args, 0, item);
                Ty::Bool
            }
            "for_each" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                Ty::Unit
            }
            "count" | "len" | "size_hint" => Ty::usize(),
            "is_empty" => Ty::Bool,
            "join" => {
                self.walk_all(args);
                Ty::Str
            }
            "sorted" | "to_vec" => Ty::vec(item.clone()),
            _ => self.walk_all(args),
        }
    }

    /// The reductions, what an iterator collapses into.
    fn iter_reduce(
        &mut self,
        item: &Ty,
        name: &str,
        args: &[&Expr],
        turbofish: Option<Ty>,
        expected: &Ty,
    ) -> Option<Ty> {
        Some(match name {
            "collect" => {
                let target = turbofish.unwrap_or_else(|| expected.clone());
                match target {
                    Ty::Str | Ty::Set(_) | Ty::Map(..) | Ty::Result(..) | Ty::Option(_) => {
                        self.vars.unify(&target.item(), item);
                        target
                    }
                    Ty::Vec(inner) => Ty::vec(self.vars.meet(&inner, item)),
                    // a target nothing names is decided later by `rustc`, guessing a vec here
                    // would beat the branch that does name it
                    _ => Ty::Unknown,
                }
            }
            "sum" | "product" => {
                let target = turbofish.unwrap_or_else(|| expected.clone());
                match target {
                    Ty::Unknown => item.clone(),
                    known => self.vars.meet(&known, item),
                }
            }
            "fold" => {
                let init = self.arg_ty(args, 0, expected);
                let got =
                    self.closure_ret_expecting(args, 1, vec![init.clone(), item.clone()], &init);
                self.vars.meet(&init, &got)
            }
            "partition" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                Ty::Tuple(vec![Ty::vec(item.clone()), Ty::vec(item.clone())])
            }
            "unzip" => match item {
                Ty::Tuple(pair) if pair.len() == 2 => {
                    Ty::Tuple(vec![Ty::vec(pair[0].clone()), Ty::vec(pair[1].clone())])
                }
                _ => Ty::Unknown,
            },
            _ => return None,
        })
    }

    /// The methods that hand back one item, or what a closure makes of one.
    fn iter_pick(&mut self, item: &Ty, name: &str, args: &[&Expr]) -> Ty {
        if name == "find_map" {
            return match args.first() {
                Some(_) => self.closure_ret(args, 0, vec![item.clone()]),
                None => Ty::Unknown,
            };
        }
        let want = match name {
            "nth" | "nth_back" => Ty::usize(),
            "reduce" | "min_by" | "max_by" => {
                Ty::Closure(vec![item.clone(), item.clone()], Box::new(Ty::Unknown))
            }
            _ => Ty::Closure(vec![item.clone()], Box::new(Ty::Unknown)),
        };
        for arg in args {
            self.expr(arg, &want);
        }
        Ty::option(item.clone())
    }
}
