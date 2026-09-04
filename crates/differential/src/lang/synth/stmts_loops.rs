//! Loop, branch and mutation statement generation.

use rand::RngExt;

use crate::lang::expr::{Expr, ReadMode};
use crate::lang::fmt::{Align, FmtSpec, FmtTrait};
use crate::lang::stmt::{MutOp, PrintForm, Stmt};
use crate::lang::synth::Generator;
use crate::lang::ty::Ty;

impl Generator<'_> {
    pub(super) fn if_stmt(&mut self) -> Stmt {
        let condition = self.expr(&Ty::Bool, 2);
        self.begin_branches();
        let then_body = self.branch(Self::nested_body);
        let else_body = self.branch(Self::nested_body);
        self.end_branches();
        Stmt::If {
            condition,
            then_body,
            else_body,
        }
    }

    pub(super) fn for_stmt(&mut self) -> Stmt {
        let count = self.rng.random_range(0..=3);
        let (body, label) = self.loop_body();
        // the counter is never read, a name would make every program warn
        Stmt::ForRange {
            var: "_".to_string(),
            count,
            body,
            label,
        }
    }

    pub(super) fn while_stmt(&mut self) -> Stmt {
        let counter = self.fresh("diff_i");
        let limit = self.rng.random_range(0..=4);
        let (body, label) = self.loop_body();
        if self.chance(0.5) {
            Stmt::While {
                counter,
                limit,
                body,
                label,
            }
        } else {
            Stmt::Loop {
                counter,
                limit,
                body,
                label,
            }
        }
    }

    /// A labeled exit may name any loop around it, so a nested loop exits through its parent.
    pub(super) fn break_or_continue(&mut self) -> Stmt {
        let condition = self.expr(&Ty::Bool, 2);
        let label = if !self.loop_labels.is_empty() && self.chance(0.5) {
            let labels = self.loop_labels.clone();
            Some(self.pick(&labels).clone())
        } else {
            None
        };
        if self.chance(0.5) {
            Stmt::Break { condition, label }
        } else {
            Stmt::Continue { condition, label }
        }
    }

    pub(super) fn return_stmt(&mut self) -> Stmt {
        let ret = self.fn_ret.clone().unwrap_or(Ty::Bool);
        let condition = self.expr(&Ty::Bool, 2);
        let value = self.expr(&ret, 2);
        Stmt::Return { condition, value }
    }

    /// A nested block. Its own `let`s shadow and drop at its end, so the caller opens a scope
    /// around it, see `scoped`.
    pub(super) fn nested_body(&mut self) -> Vec<Stmt> {
        let count = self.rng.random_range(1..=3);
        let mut body = Vec::new();
        for _ in 0..count {
            let stmt = self.statement(|inner| match inner.rng.random_range(0..8) {
                0 => inner.assign_stmt(),
                1 => inner.compound_stmt().unwrap_or_else(|| inner.observation()),
                2 => inner
                    .collection_mutation()
                    .unwrap_or_else(|| inner.observation()),
                3 if inner.in_loop => inner.break_or_continue(),
                4 if inner.fn_ret.is_some() => inner.return_stmt(),
                5 | 6 => inner.binding_stmt(),
                _ => inner.observation(),
            });
            body.push(stmt);
        }
        body
    }

    /// The body and the loop's label. A label nobody names is dropped, it would only warn.
    fn loop_body(&mut self) -> (Vec<Stmt>, Option<String>) {
        let was = std::mem::replace(&mut self.in_loop, true);
        let label = self.chance(0.5).then(|| self.fresh("diff_l"));
        if let Some(label) = &label {
            self.loop_labels.push(label.clone());
        }
        let body = self.looping(Self::nested_body);
        if label.is_some() {
            self.loop_labels.pop();
        }
        self.in_loop = was;
        let label = label.filter(|l| body.iter().any(|stmt| stmt.targets_label(l)));
        (body, label)
    }

    /// `name` is hidden because an `entry()` chain holds the map while its arguments evaluate.
    fn op_without(&mut self, name: &str, build: impl FnOnce(&mut Self) -> MutOp) -> MutOp {
        self.without_binding(name, build)
    }

    /// The binding is borrowed for the whole call, so the arguments may read it but not take
    /// it. That is the two phase borrow `rustc` allows a `push` of `v[0].clone()`.
    pub(super) fn collection_mutation(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_mutable()?;
        let op = self.holding(&name.clone(), |inner| inner.collection_op(&name, &ty));
        Some(Stmt::Mutate { name, op })
    }

    fn collection_op(&mut self, name: &str, ty: &Ty) -> MutOp {
        match ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                match self.rng.random_range(0..11) {
                    0 => MutOp::VecPush(self.expr(&elem, 1)),
                    1 if elem.is_ord() => MutOp::VecSort,
                    2 => match &elem {
                        Ty::Vec(inner) => MutOp::VecRowPush {
                            index: self.rng.random_range(0..=2),
                            value: self.expr(inner, 1),
                        },
                        _ => MutOp::VecDedup,
                    },
                    3 => MutOp::VecSetIndex {
                        index: self.rng.random_range(0..=6),
                        value: self.expr(&elem, 1),
                    },
                    4 => MutOp::VecReverse,
                    5 => MutOp::VecPop,
                    6 => MutOp::VecTruncate(self.rng.random_range(0..=3)),
                    7 => MutOp::VecSwap(self.rng.random_range(0..=4), self.rng.random_range(0..=4)),
                    8 => {
                        let bind = self.fresh("diff_r");
                        let locals = [(bind.clone(), elem.clone())];
                        // `retain` holds the vec, so the predicate must not read it
                        let pred = self.without_binding(name, |inner| {
                            inner.closure_body(|inner| {
                                inner.with_locals(&locals, |inner| inner.expr(&Ty::Bool, 1))
                            })
                        });
                        MutOp::VecRetain { bind, pred }
                    }
                    9 => MutOp::VecClear,
                    _ => MutOp::VecExtend(self.expr(&Ty::vec_of(elem), 1)),
                }
            }
            Ty::Str => match self.rng.random_range(0..3) {
                0 => MutOp::StrPush(self.expr(&Ty::Char, 1)),
                1 => MutOp::StrClear,
                _ => MutOp::StrPushStr(self.expr(&Ty::Str, 1)),
            },
            Ty::Opt(elem) => {
                if self.chance(0.5) {
                    MutOp::OptTake
                } else {
                    MutOp::OptReplace(self.expr(elem, 1))
                }
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                match (&val_ty, self.rng.random_range(0..4)) {
                    (Ty::Int(_), 0) => self.op_without(name, |inner| MutOp::MapEntryAdd {
                        key: inner.expr(&key_ty, 1),
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    (Ty::Vec(elem), 0) => {
                        let elem = (**elem).clone();
                        self.op_without(name, |inner| MutOp::MapEntryPush {
                            key: inner.expr(&key_ty, 1),
                            value: inner.expr(&elem, 1),
                        })
                    }
                    (_, 1) => MutOp::MapRemove {
                        key: self.expr(&key_ty, 1),
                    },
                    _ => MutOp::MapInsert {
                        key: self.expr(&key_ty, 1),
                        value: self.expr(&val_ty, 1),
                    },
                }
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                if self.chance(0.7) {
                    MutOp::SetInsert(self.expr(&elem, 1))
                } else {
                    MutOp::SetRemove(self.expr(&elem, 1))
                }
            }
            _ => unreachable!("pick_mutable offers only the types above"),
        }
    }

    /// `for item in vec { accumulate into collection }`. The source is a vec, so order is
    /// defined. The item is moved into the target, it is the loop's own binding.
    pub(super) fn accumulation_loop(&mut self) -> Option<Stmt> {
        let (target, ty) = self.pick_collection()?;
        let var = self.fresh("diff_item");
        let source_elem = match &ty {
            Ty::Vec(elem) | Ty::Set(elem) => (**elem).clone(),
            Ty::Map(key, _) => (**key).clone(),
            _ => return None,
        };
        // the target is held by the loop, so the source can't take it
        let source = self.holding(&target, |inner| {
            inner.expr(&Ty::vec_of(source_elem.clone()), 2)
        });
        let item = Expr::Var {
            name: var.clone(),
            ty: source_elem.clone(),
            mode: ReadMode::Move,
        };
        let op = self.holding(&target.clone(), |inner| {
            inner.looping(|inner| {
                inner.push_local(var.clone(), source_elem.clone());
                // the item goes first into the op, so the other arguments can't read it
                if !source_elem.is_copy() {
                    inner.scope.note_move(&var);
                }
                inner.accumulate_op(&target, &ty, item)
            })
        });
        Some(Stmt::ForAccum {
            var,
            source,
            target,
            op,
        })
    }

    fn accumulate_op(&mut self, target: &str, ty: &Ty, item: Expr) -> MutOp {
        match ty {
            Ty::Vec(_) => MutOp::VecPush(item),
            Ty::Set(_) => MutOp::SetInsert(item),
            Ty::Map(_, value) => {
                let val_ty = (**value).clone();
                match &val_ty {
                    Ty::Int(_) => self.op_without(target, |inner| MutOp::MapEntryAdd {
                        key: item,
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    Ty::Vec(elem) => {
                        let elem = (**elem).clone();
                        self.op_without(target, |inner| MutOp::MapEntryPush {
                            key: item,
                            value: inner.expr(&elem, 1),
                        })
                    }
                    _ => MutOp::MapInsert {
                        key: item,
                        value: self.expr(&val_ty, 1),
                    },
                }
            }
            _ => unreachable!("pick_collection offers only the types above"),
        }
    }

    /// `for r in vec.iter_mut() { *r = expr(r) }` on a vec binding.
    pub(super) fn for_mut_stmt(&mut self) -> Option<Stmt> {
        let vecs: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter_map(|(name, ty)| match ty {
                Ty::Vec(elem) => Some((name, *elem)),
                _ => None,
            })
            .collect();
        if vecs.is_empty() {
            return None;
        }
        let (name, elem) = self.pick(&vecs).clone();
        let var = self.fresh("diff_e");
        // the vec is borrowed for the loop, so its name is hidden
        let expr = self.without_binding(&name, |inner| {
            inner.looping(|inner| {
                inner.push_local(var.clone(), elem.clone());
                inner.expr(&elem, 2)
            })
        });
        Some(Stmt::ForMut {
            name,
            var,
            elem,
            expr,
        })
    }

    /// `helper(&mut binding, args)`
    pub(super) fn call_mut_stmt(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_local()?;
        let (fn_name, params) = self.writer_fn(&ty);
        // the target is borrowed for the call, so the arguments can't read it
        let args = self.without_binding(&name, |inner| {
            params.iter().map(|param| inner.expr(param, 1)).collect()
        });
        Some(Stmt::CallMut {
            name,
            fn_name,
            args,
        })
    }

    fn pick_mutable(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(_, ty)| {
                matches!(
                    ty,
                    Ty::Vec(_) | Ty::Map(..) | Ty::Set(_) | Ty::Str | Ty::Opt(_)
                )
            })
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }

    fn pick_collection(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(_, ty)| matches!(ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_)))
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }

    // observations

    pub(super) fn print_stmt(&mut self, expr: Expr) -> Stmt {
        let label = self.next_label();
        let observed = match expr.ty() {
            Ty::Map(key, value) => Ty::vec_of(Ty::Tuple(vec![*key, *value])),
            Ty::Set(elem) => Ty::vec_of(*elem),
            other => other,
        };
        let spec = self.fmt_spec(&observed);
        let form = match self.rng.random_range(0..10) {
            0 => PrintForm::Indexed,
            1 => PrintForm::Twice,
            2 => PrintForm::WidthArg(self.rng.random_range(0..=12)),
            3 => PrintForm::NamedWidth(self.rng.random_range(0..=12)),
            _ => PrintForm::Plain,
        };
        Stmt::Print {
            label,
            expr,
            spec,
            form,
        }
    }

    /// Plain `{:?}` a third of the time so observations stay readable.
    pub(super) fn fmt_spec(&mut self, ty: &Ty) -> FmtSpec {
        if self.chance(0.35) {
            return FmtSpec::DEBUG;
        }
        for _ in 0..6 {
            let fmt = *self.pick(&[
                FmtTrait::Display,
                FmtTrait::Display,
                FmtTrait::Debug,
                FmtTrait::Debug,
                FmtTrait::LowerHex,
                FmtTrait::UpperHex,
                FmtTrait::Octal,
                FmtTrait::Binary,
                FmtTrait::LowerExp,
                FmtTrait::UpperExp,
            ]);
            let mut spec = FmtSpec::plain(fmt);
            if self.chance(0.5) {
                spec.width = Some(self.rng.random_range(0..=12));
            }
            if self.chance(0.45) {
                spec.align = Some(*self.pick(&[Align::Left, Align::Right, Align::Center]));
                if self.chance(0.6) {
                    spec.fill = Some(*self.pick(&['*', '-', '=', '_', '~', '0']));
                }
            }
            spec.plus = self.chance(0.2);
            spec.zero = self.chance(0.2);
            spec.alternate = self.chance(0.25);
            if self.chance(0.3) {
                spec.precision = Some(self.rng.random_range(0..=4));
            }
            if spec.applies_to(ty) {
                return spec;
            }
        }
        FmtSpec::DEBUG
    }
}
