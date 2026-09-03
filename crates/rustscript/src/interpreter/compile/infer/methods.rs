//! Method calls. The receiver type picks the table, the table gives each argument its expected
//! type and the result. A method not in a table gives `Unknown` after walking its arguments.

use syn::Expr;

use super::{Infer, Ty};
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    pub(super) fn method_call(&mut self, m: &syn::ExprMethodCall, expected: &Ty) -> Ty {
        let name = m.method.to_string();
        let recv_want = receiver_expectation(&name, expected);
        let recv = self.expr(&m.receiver, &recv_want);
        let turbofish = m
            .turbofish
            .as_ref()
            .and_then(|t| {
                t.args.iter().find_map(|a| match a {
                    syn::GenericArgument::Type(ty) => Some(ty),
                    _ => None,
                })
            })
            .map(|t| self.lower(t));
        let args: Vec<&Expr> = m.args.iter().collect();
        // a script type answers from its own impl
        if let Ty::Struct(canon) | Ty::Enum(canon) = &recv
            && let Some(sig) = self
                .ctx
                .impl_sigs
                .get(&(canon.to_string(), name.clone()))
                .cloned()
        {
            let mut full: Vec<&Expr> = vec![&m.receiver];
            full.extend(args.iter().copied());
            return self.sig_call(&sig, Some(&recv), &full, expected);
        }
        // a trait method on a bridge type, `impl Describe for Vec<u8>`
        if self.ctx.method_atoms.contains_key(&name)
            && let Some(sig) = self
                .ctx
                .impl_sigs
                .iter()
                .find(|((_, n), _)| *n == name)
                .map(|(_, sig)| sig.clone())
        {
            let mut full: Vec<&Expr> = vec![&m.receiver];
            full.extend(args.iter().copied());
            return self.sig_call(&sig, Some(&recv), &full, expected);
        }
        if let Some(ty) = self.common_method(&recv, &name, &args, turbofish.as_ref(), expected) {
            return ty;
        }
        match &recv {
            Ty::Str => self.str_method(&name, &args, turbofish, expected),
            Ty::Vec(item) => {
                let item = (**item).clone();
                self.vec_method(&recv, &item, &name, &args, turbofish, expected)
            }
            Ty::Set(item) => {
                let item = (**item).clone();
                self.set_method(&recv, &item, &name, &args)
            }
            Ty::Map(key, value) => {
                let (key, value) = ((**key).clone(), (**value).clone());
                self.map_method(&recv, &key, &value, &name, &args)
            }
            Ty::Entry(value) => {
                let value = (**value).clone();
                self.entry_method(&recv, &value, &name, &args)
            }
            Ty::Option(payload) => {
                let payload = (**payload).clone();
                self.option_method(&recv, &payload, &name, &args, expected)
            }
            Ty::Result(ok, err) => {
                let (ok, err) = ((**ok).clone(), (**err).clone());
                self.result_method(&recv, &ok, &err, &name, &args, expected)
            }
            Ty::Iter(item) | Ty::Range(item) => {
                let item = (**item).clone();
                self.iter_method(&recv, &item, &name, &args, turbofish, expected)
            }
            Ty::Int(_) | Ty::IntVar(_) => self.int_method(&recv, &name, &args),
            Ty::F32 | Ty::F64 | Ty::FloatVar(_) => self.float_method(&recv, &name, &args),
            Ty::Char => self.char_method(&name, &args),
            Ty::Json => self.json_method(&name, &args),
            Ty::Bool => self.bool_method(&name, &args, expected),
            Ty::Named(kind, _) => {
                let kind = kind.clone();
                self.named_method(&kind, &name, &args, expected)
            }
            _ => self.walk_all(&args),
        }
    }

    pub(super) fn walk_all(&mut self, args: &[&Expr]) -> Ty {
        for arg in args {
            self.expr(arg, &Ty::Unknown);
        }
        Ty::Unknown
    }

    pub(super) fn arg_ty(&mut self, args: &[&Expr], i: usize, want: &Ty) -> Ty {
        match args.get(i) {
            Some(arg) => self.expr(arg, want),
            None => Ty::Unknown,
        }
    }

    /// The return type of a closure argument given its parameter types.
    pub(super) fn closure_ret(&mut self, args: &[&Expr], i: usize, params: Vec<Ty>) -> Ty {
        let want = Ty::Closure(params, Box::new(Ty::Unknown));
        match self.arg_ty(args, i, &want) {
            Ty::Closure(_, ret) => *ret,
            _ => Ty::Unknown,
        }
    }

    /// Methods every receiver has.
    fn common_method(
        &mut self,
        recv: &Ty,
        name: &str,
        args: &[&Expr],
        turbofish: Option<&Ty>,
        expected: &Ty,
    ) -> Option<Ty> {
        Some(match name {
            "clone" | "to_owned" | "borrow" | "borrow_mut" | "lock" | "as_ref" | "as_mut"
            | "deref" | "into_inner" | "by_ref"
                if args.is_empty()
                    && !matches!(recv, Ty::Option(_) | Ty::Result(_, _) | Ty::Iter(_)) =>
            {
                recv.clone()
            }
            "to_string" => {
                self.walk_all(args);
                Ty::Str
            }
            "into" if args.is_empty() => {
                if expected.is_unknown() {
                    match recv {
                        Ty::Unknown => Ty::Unknown,
                        other => other.clone(),
                    }
                } else {
                    expected.clone()
                }
            }
            "try_into" if args.is_empty() => Ty::result(
                turbofish.cloned().unwrap_or_else(|| expected.payload()),
                Ty::named("TryFromIntError"),
            ),
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                self.arg_ty(args, 0, recv);
                Ty::Bool
            }
            "cmp" | "partial_cmp" | "total_cmp" => {
                self.arg_ty(args, 0, recv);
                if name == "partial_cmp" {
                    Ty::option(Ty::named("Ordering"))
                } else {
                    Ty::named("Ordering")
                }
            }
            "hash" | "fmt" => {
                self.walk_all(args);
                Ty::Unknown
            }
            "set" if matches!(recv, Ty::Unknown) => return None,
            _ => return None,
        })
    }

    fn str_method(
        &mut self,
        name: &str,
        args: &[&Expr],
        turbofish: Option<Ty>,
        expected: &Ty,
    ) -> Ty {
        match name {
            "len" | "capacity" => Ty::usize(),
            "is_empty"
            | "contains"
            | "starts_with"
            | "ends_with"
            | "is_ascii"
            | "is_char_boundary"
            | "eq_ignore_ascii_case" => {
                self.walk_all(args);
                Ty::Bool
            }
            "chars" | "into_iter" | "iter" => Ty::iter(Ty::Char),
            "bytes" | "as_bytes" | "into_bytes" => {
                if name == "bytes" {
                    Ty::iter(Ty::Int(IntWidth::U8))
                } else {
                    Ty::vec(Ty::Int(IntWidth::U8))
                }
            }
            "char_indices" => Ty::iter(Ty::Tuple(vec![Ty::usize(), Ty::Char])),
            "lines"
            | "split"
            | "rsplit"
            | "splitn"
            | "rsplitn"
            | "split_terminator"
            | "split_whitespace"
            | "split_ascii_whitespace"
            | "split_inclusive"
            | "matches" => {
                self.walk_all(args);
                Ty::iter(Ty::Str)
            }
            "find" | "rfind" => {
                self.walk_all(args);
                Ty::option(Ty::usize())
            }
            "split_once" | "rsplit_once" => {
                self.walk_all(args);
                Ty::option(Ty::Tuple(vec![Ty::Str, Ty::Str]))
            }
            "split_at" => {
                self.arg_ty(args, 0, &Ty::usize());
                Ty::Tuple(vec![Ty::Str, Ty::Str])
            }
            "strip_prefix" | "strip_suffix" | "get" => {
                self.walk_all(args);
                Ty::option(Ty::Str)
            }
            "parse" => {
                let target = turbofish.unwrap_or_else(|| expected.payload());
                Ty::result(target, Ty::named("ParseError"))
            }
            "push" => {
                self.arg_ty(args, 0, &Ty::Char);
                Ty::Unit
            }
            "push_str"
            | "clear"
            | "truncate"
            | "insert"
            | "insert_str"
            | "retain"
            | "make_ascii_uppercase"
            | "make_ascii_lowercase"
            | "reserve"
            | "extend" => {
                self.walk_all(args);
                Ty::Unit
            }
            "pop" => Ty::option(Ty::Char),
            "remove" => {
                self.walk_all(args);
                Ty::Char
            }
            "to_lowercase" | "to_uppercase" | "to_ascii_lowercase" | "to_ascii_uppercase"
            | "trim" | "trim_start" | "trim_end" | "trim_matches" | "trim_start_matches"
            | "trim_end_matches" | "replace" | "replacen" | "repeat" | "as_str" | "to_owned"
            | "clone" | "into_string" | "to_string_lossy" | "into_boxed_str" | "escape_default"
            | "to_str" => {
                self.walk_all(args);
                if name == "to_str" {
                    Ty::option(Ty::Str)
                } else {
                    Ty::Str
                }
            }
            "bold" | "red" | "green" | "yellow" | "blue" | "cyan" | "magenta" | "white"
            | "dimmed" | "italic" | "underline" | "normal" | "bright_black" | "bright_red"
            | "bright_green" | "bright_yellow" | "bright_blue" | "bright_magenta"
            | "bright_cyan" | "bright_white" | "black" | "on_red" | "on_green" | "on_blue"
            | "on_yellow" | "strikethrough" => Ty::Str,
            _ => self.walk_all(args),
        }
    }

    /// Reads and pushes on a vec.
    fn vec_access(&mut self, recv: &Ty, item: &Ty, name: &str, args: &[&Expr]) -> Option<Ty> {
        Some(match name {
            "len" | "capacity" => Ty::usize(),
            "is_empty" | "contains" | "starts_with" | "ends_with" | "is_sorted" => {
                self.arg_ty(args, 0, item);
                Ty::Bool
            }
            "iter" | "into_iter" | "iter_mut" | "drain" | "into_values" => {
                self.walk_all(args);
                Ty::iter(item.clone())
            }
            "push" | "push_back" | "push_front" => {
                self.arg_ty(args, 0, item);
                Ty::Unit
            }
            "insert" => {
                self.arg_ty(args, 0, &Ty::usize());
                self.arg_ty(args, 1, item);
                Ty::Unit
            }
            "pop" | "pop_back" | "pop_front" | "first" | "last" | "get" | "get_mut"
            | "first_mut" | "last_mut" => {
                self.walk_all(args);
                Ty::option(item.clone())
            }
            "remove" | "swap_remove" => {
                self.arg_ty(args, 0, &Ty::usize());
                item.clone()
            }
            "to_vec" | "clone" | "as_slice" | "as_mut_slice" | "into_boxed_slice" | "to_owned" => {
                recv.clone()
            }
            _ => return None,
        })
    }

    fn vec_method(
        &mut self,
        recv: &Ty,
        item: &Ty,
        name: &str,
        args: &[&Expr],
        turbofish: Option<Ty>,
        expected: &Ty,
    ) -> Ty {
        if let Some(ty) = self.vec_access(recv, item, name, args) {
            return ty;
        }
        match name {
            "extend" | "extend_from_slice" | "append" | "sort" | "sort_unstable" | "reverse"
            | "dedup" | "truncate" | "clear" | "swap" | "resize" | "fill" | "rotate_left"
            | "rotate_right" | "shrink_to_fit" | "reserve" | "copy_from_slice"
            | "clone_from_slice" => {
                for (i, arg) in args.iter().enumerate() {
                    let want = if (name == "resize" && i == 1) || name == "fill" {
                        item.clone()
                    } else {
                        Ty::Unknown
                    };
                    self.expr(arg, &want);
                }
                Ty::Unit
            }
            "sort_by" | "sort_unstable_by" => {
                self.closure_ret(args, 0, vec![item.clone(), item.clone()]);
                Ty::Unit
            }
            "sort_by_key"
            | "sort_unstable_by_key"
            | "sort_by_cached_key"
            | "retain"
            | "retain_mut"
            | "dedup_by_key" => {
                self.closure_ret(args, 0, vec![item.clone()]);
                Ty::Unit
            }
            "join" => {
                self.walk_all(args);
                Ty::Str
            }
            "concat" => match item {
                Ty::Vec(inner) => Ty::vec((**inner).clone()),
                Ty::Str => Ty::Str,
                _ => Ty::Unknown,
            },
            "split_first" | "split_last" => Ty::option(Ty::Tuple(vec![item.clone(), recv.clone()])),
            "split_at" => {
                self.arg_ty(args, 0, &Ty::usize());
                Ty::Tuple(vec![recv.clone(), recv.clone()])
            }
            "split_off" | "repeat" => {
                self.arg_ty(args, 0, &Ty::usize());
                recv.clone()
            }
            "windows" | "chunks" | "chunks_exact" | "rchunks" => {
                self.arg_ty(args, 0, &Ty::usize());
                Ty::iter(recv.clone())
            }
            "binary_search" => {
                self.arg_ty(args, 0, item);
                Ty::result(Ty::usize(), Ty::usize())
            }
            _ => self.iter_method(recv, item, name, args, turbofish, expected),
        }
    }

    fn set_method(&mut self, recv: &Ty, item: &Ty, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "len" => Ty::usize(),
            "insert" | "contains" | "remove" | "is_empty" | "is_subset" | "is_superset"
            | "is_disjoint" => {
                self.arg_ty(args, 0, item);
                Ty::Bool
            }
            "iter" | "into_iter" | "drain" => Ty::iter(item.clone()),
            "union" | "intersection" | "difference" | "symmetric_difference" => {
                self.arg_ty(args, 0, recv);
                Ty::iter(item.clone())
            }
            "get" | "take" | "first" | "last" | "pop_first" | "pop_last" => {
                self.walk_all(args);
                Ty::option(item.clone())
            }
            "extend" | "clear" | "retain" => {
                self.walk_all(args);
                Ty::Unit
            }
            "clone" => recv.clone(),
            _ => self.iter_method(recv, item, name, args, None, &Ty::Unknown),
        }
    }

    fn map_method(&mut self, recv: &Ty, key: &Ty, value: &Ty, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "len" => Ty::usize(),
            "is_empty" => Ty::Bool,
            "contains_key" => {
                self.arg_ty(args, 0, key);
                Ty::Bool
            }
            "get" | "get_mut" | "remove" | "get_key_value" => {
                self.arg_ty(args, 0, key);
                if name == "get_key_value" {
                    Ty::option(Ty::Tuple(vec![key.clone(), value.clone()]))
                } else {
                    Ty::option(value.clone())
                }
            }
            "insert" => {
                self.arg_ty(args, 0, key);
                self.arg_ty(args, 1, value);
                Ty::option(value.clone())
            }
            "entry" => {
                self.arg_ty(args, 0, key);
                Ty::Entry(Box::new(value.clone()))
            }
            "keys" | "into_keys" => Ty::iter(key.clone()),
            "values" | "values_mut" | "into_values" => Ty::iter(value.clone()),
            "iter" | "iter_mut" | "into_iter" | "drain" => {
                Ty::iter(Ty::Tuple(vec![key.clone(), value.clone()]))
            }
            "extend" | "clear" | "retain" => {
                self.walk_all(args);
                Ty::Unit
            }
            "first_key_value" | "last_key_value" | "pop_first" | "pop_last" => {
                Ty::option(Ty::Tuple(vec![key.clone(), value.clone()]))
            }
            "clone" => recv.clone(),
            _ => self.walk_all(args),
        }
    }

    fn entry_method(&mut self, recv: &Ty, value: &Ty, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "or_insert" => {
                self.arg_ty(args, 0, value);
                value.clone()
            }
            "or_insert_with" => {
                self.closure_ret(args, 0, Vec::new());
                value.clone()
            }
            "or_default" => value.clone(),
            "and_modify" => {
                self.closure_ret(args, 0, vec![value.clone()]);
                recv.clone()
            }
            "key" => Ty::Unknown,
            _ => self.walk_all(args),
        }
    }
}

/// What the receiver of a method is expected to be, from what the call is expected to give. An
/// unwrap wants a payload carrier around the expectation, an error mapper keeps the `Ok` side.
fn receiver_expectation(name: &str, expected: &Ty) -> Ty {
    if expected.is_unknown() {
        return Ty::Unknown;
    }
    match name {
        "unwrap" | "expect" | "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" => {
            Ty::option(expected.clone())
        }
        "map_err" | "context" | "with_context" | "inspect_err" | "or_else" | "inspect" => {
            match expected {
                Ty::Result(ok, _) => Ty::result((**ok).clone(), Ty::Unknown),
                _ => Ty::Unknown,
            }
        }
        "ok" => match expected {
            Ty::Option(t) => Ty::result((**t).clone(), Ty::Unknown),
            _ => Ty::Unknown,
        },
        _ => Ty::Unknown,
    }
}
