//! Top level item generation.

use rand::RngExt;

use crate::lang::block::{ConstDef, FnDef, FnKind, Param, ParamMode};
use crate::lang::expr::{Arm, BinOp, Expr};
use crate::lang::pat::Pat;
use crate::lang::stmt::{Ann, Stmt};
use crate::lang::synth::{BindKind, Binding, Generator};
use crate::lang::ty::{MAX_TY_DEPTH, StdErr, Ty};
use crate::lang::user::{
    Compare, Derives, DisplayForm, DisplayImpl, DisplayPiece, Field, FromImpl, MethodKind,
    MethodSig, Ret, UserDef, UserKind, UserMethod, UserShape, Variant,
};

impl Generator<'_> {
    // -- user types -----------------------------------------------------------

    pub(super) fn declare_types(&mut self) {
        let count = self.rng.random_range(0..=3);
        for index in 0..count {
            let def = match self.rng.random_range(0..3) {
                0 => self.struct_def(index),
                1 => self.enum_def(index, false),
                _ => self.enum_def(index, true),
            };
            self.types.push(def);
        }
    }

    /// Mostly scalars, sometimes a container, a tuple, a std error or an
    /// earlier user type.
    fn member_ty(&mut self, error: bool) -> Ty {
        if error && self.chance(0.4) {
            return Ty::StdErr(if self.chance(0.7) {
                StdErr::ParseInt
            } else {
                StdErr::ParseFloat
            });
        }
        for _ in 0..4 {
            let candidate = match self.rng.random_range(0..10) {
                0 => Ty::vec_of(self.scalar_ty()),
                1 => Ty::opt_of(self.scalar_ty()),
                2 => Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]),
                3 => match self.user_ty() {
                    Some(ty) => ty,
                    None => self.scalar_ty(),
                },
                _ => self.scalar_ty(),
            };
            if candidate.depth() < MAX_TY_DEPTH {
                return candidate;
            }
        }
        self.scalar_ty()
    }

    /// Each derive is dropped now and then so a type without it exists too.
    fn derives_for(&mut self, members: &[Ty]) -> Derives {
        let eq = members.iter().all(Ty::is_eq) && self.chance(0.9);
        let ord = eq && members.iter().all(Ty::is_ord) && self.chance(0.85);
        Derives {
            compare: match (eq, ord) {
                (_, true) => Compare::Ord,
                (true, false) => Compare::Eq,
                (false, false) => Compare::Partial,
            },
            hash: eq && members.iter().all(Ty::is_hash) && self.chance(0.85),
            default: members.iter().all(Ty::has_default) && self.chance(0.8),
        }
    }

    fn struct_def(&mut self, index: usize) -> UserDef {
        let name = format!("DiffS{}_{index}", self.tag);
        let count = self.rng.random_range(1..=3);
        let fields: Vec<Field> = (0..count)
            .map(|slot| Field {
                name: format!("f{slot}"),
                ty: self.member_ty(false),
            })
            .collect();
        let members: Vec<Ty> = fields.iter().map(|field| field.ty.clone()).collect();
        let derives = self.derives_for(&members);
        let display = self.chance(0.5);
        let describe = self.chance(0.3);
        let depth = members.iter().map(Ty::depth).max().unwrap_or(0);
        let has_float = members.iter().any(Ty::contains_float);
        let mut shape = UserShape {
            name,
            kind: UserKind::Struct(fields.clone()),
            derives,
            display,
            describe,
            methods: Vec::new(),
            froms: Vec::new(),
            depth,
            has_float,
        };
        // From<the first scalar field>.
        let from_slot = fields
            .iter()
            .position(|field| matches!(field.ty, Ty::Int(_) | Ty::Str | Ty::Bool | Ty::Char));
        let mut froms = Vec::new();
        if let Some(slot) = from_slot
            && self.chance(0.6)
        {
            let src = fields[slot].ty.clone();
            let rest = if derives.default {
                Vec::new()
            } else {
                fields
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != slot)
                    .map(|(_, field)| crate::lang::expr::minimal(&field.ty))
                    .collect()
            };
            shape.froms.push(src.clone());
            froms.push(FromImpl { src, slot, rest });
        }
        let display_impl = display.then(|| {
            let pieces = fields
                .iter()
                .map(|field| DisplayPiece {
                    spec: self.fmt_spec(&field.ty),
                })
                .collect();
            DisplayImpl {
                form: if self.chance(0.5) {
                    DisplayForm::Write
                } else {
                    DisplayForm::Pad
                },
                pieces: vec![pieces],
            }
        });
        let methods = self.struct_methods(&mut shape, &fields);
        UserDef {
            shape,
            display: display_impl,
            methods,
            froms,
        }
    }

    fn struct_methods(&mut self, shape: &mut UserShape, fields: &[Field]) -> Vec<UserMethod> {
        let owner = Ty::user(shape.clone());
        let mut methods = Vec::new();
        // A local named `self.f0` renders exactly like a field read.
        let self_locals: Vec<(String, Ty)> = fields
            .iter()
            .map(|field| (format!("self.{}", field.name), field.ty.clone()))
            .collect();
        let count = self.rng.random_range(0..=2);
        for slot in 0..count {
            let ret = if self.chance(0.3) {
                Ret::Same
            } else {
                Ret::Ty(self.scalar_ty())
            };
            let arg_count = self.rng.random_range(0..=1);
            let params: Vec<(String, Ty)> = (0..arg_count)
                .map(|_| (self.fresh("diff_a"), self.scalar_ty()))
                .collect();
            let ret_ty = match &ret {
                Ret::Same => owner.clone(),
                Ret::Ty(ty) => ty.clone(),
            };
            let body = self.without_scope(|inner| {
                inner.with_locals(&self_locals, |inner| {
                    inner.with_locals(&params, |inner| inner.expr(&ret_ty, 2))
                })
            });
            let sig = MethodSig {
                name: format!("diff_m{slot}"),
                kind: MethodKind::Method,
                args: params.iter().map(|(_, ty)| ty.clone()).collect(),
                ret,
            };
            shape.methods.push(sig.clone());
            methods.push(UserMethod {
                sig,
                params: params.into_iter().map(|(name, _)| name).collect(),
                body,
            });
        }
        if self.chance(0.5) {
            let params: Vec<(String, Ty)> = fields
                .iter()
                .take(2)
                .map(|field| (self.fresh("diff_a"), field.ty.clone()))
                .collect();
            let body = self.without_scope(|inner| {
                inner.with_locals(&params, |inner| {
                    let values = fields
                        .iter()
                        .enumerate()
                        .map(|(index, field)| match params.get(index) {
                            Some((name, ty)) => Expr::Var {
                                name: name.clone(),
                                ty: ty.clone(),
                            },
                            None => inner.expr(&field.ty, 1),
                        })
                        .collect();
                    Expr::StructLit {
                        shape: Box::new(shape.clone()),
                        fields: values,
                        update: false,
                    }
                })
            });
            let sig = MethodSig {
                name: "diff_new".to_string(),
                kind: MethodKind::Assoc,
                args: params.iter().map(|(_, ty)| ty.clone()).collect(),
                ret: Ret::Same,
            };
            shape.methods.push(sig.clone());
            methods.push(UserMethod {
                sig,
                params: params.into_iter().map(|(name, _)| name).collect(),
                body,
            });
        }
        methods
    }

    /// With `error`, the payloads include std parse errors and `From` impls
    /// convert them.
    fn enum_def(&mut self, index: usize, error: bool) -> UserDef {
        let name = format!(
            "Diff{}{}_{index}",
            if error { "Err" } else { "E" },
            self.tag
        );
        let count = self.rng.random_range(2..=4);
        let mut variants = Vec::new();
        for slot in 0..count {
            // The first variant is a unit so a derived `Default` has one to
            // mark.
            let payload_count = if slot == 0 {
                0
            } else {
                self.rng.random_range(0..=2)
            };
            let payload = (0..payload_count).map(|_| self.member_ty(error)).collect();
            variants.push(Variant {
                name: format!("V{slot}"),
                payload,
            });
        }
        if error
            && !variants
                .iter()
                .any(|v| matches!(v.payload.as_slice(), [Ty::StdErr(_)]))
        {
            variants.push(Variant {
                name: "Parse".to_string(),
                payload: vec![Ty::StdErr(StdErr::ParseInt)],
            });
        }
        let members: Vec<Ty> = variants.iter().flat_map(|v| v.payload.clone()).collect();
        let derives = self.derives_for(&members);
        let display = self.chance(0.5) || error;
        let describe = self.chance(0.3);
        let depth = members.iter().map(Ty::depth).max().unwrap_or(0);
        let has_float = members.iter().any(Ty::contains_float);
        let mut shape = UserShape {
            name,
            kind: UserKind::Enum(variants.clone()),
            derives,
            display,
            describe,
            methods: Vec::new(),
            froms: Vec::new(),
            depth,
            has_float,
        };
        // `From<payload>` for every single payload variant, so `?` has
        // conversions to go through.
        let mut froms = Vec::new();
        for (slot, variant) in variants.iter().enumerate() {
            if let [payload] = variant.payload.as_slice()
                && !shape.froms.contains(payload)
                && (error || self.chance(0.5))
            {
                shape.froms.push(payload.clone());
                froms.push(FromImpl {
                    src: payload.clone(),
                    slot,
                    rest: Vec::new(),
                });
            }
        }
        let display_impl = display.then(|| {
            let pieces = variants
                .iter()
                .map(|variant| {
                    variant
                        .payload
                        .iter()
                        .map(|ty| DisplayPiece {
                            spec: self.fmt_spec(ty),
                        })
                        .collect()
                })
                .collect();
            DisplayImpl {
                form: if self.chance(0.5) {
                    DisplayForm::Write
                } else {
                    DisplayForm::Pad
                },
                pieces,
            }
        });
        let methods = self.enum_methods(&mut shape, &variants);
        UserDef {
            shape,
            display: display_impl,
            methods,
            froms,
        }
    }

    /// `fn diff_code(&self) -> i64`.
    fn enum_methods(&mut self, shape: &mut UserShape, variants: &[Variant]) -> Vec<UserMethod> {
        if !self.chance(0.6) {
            return Vec::new();
        }
        let owner = Ty::user(shape.clone());
        let ret = self.scalar_ty();
        let arms = self.without_scope(|inner| {
            variants
                .iter()
                .enumerate()
                .map(|(index, variant)| {
                    let payload: Vec<Pat> = variant
                        .payload
                        .iter()
                        .map(|ty| Pat::Bind {
                            name: inner.fresh("diff_b"),
                            ty: ty.clone(),
                        })
                        .collect();
                    let pat = Pat::Variant {
                        shape: Box::new(shape.clone()),
                        variant: index,
                        payload,
                    };
                    let mut binds = Vec::new();
                    pat.bindings(&mut binds);
                    let body = inner.with_locals(&binds, |inner| inner.expr(&ret, 2));
                    Arm {
                        pat,
                        guard: None,
                        body,
                    }
                })
                .collect()
        });
        let body = Expr::Match {
            scrutinee: Box::new(Expr::Var {
                name: "self".to_string(),
                ty: owner,
            }),
            by_ref: false,
            arms,
            ty: ret.clone(),
        };
        let sig = MethodSig {
            name: "diff_code".to_string(),
            kind: MethodKind::Method,
            args: Vec::new(),
            ret: Ret::Ty(ret),
        };
        shape.methods.push(sig.clone());
        vec![UserMethod {
            sig,
            params: Vec::new(),
            body,
        }]
    }

    // -- consts and trait impls -----------------------------------------------

    pub(super) fn declare_consts(&mut self) {
        let count = self.rng.random_range(0..=2);
        for _ in 0..count {
            let ty = match self.rng.random_range(0..4) {
                0 => Ty::Float(self.float_width()),
                1 => Ty::Bool,
                2 => Ty::Char,
                _ => Ty::Int(self.int_width()),
            };
            let name = format!("DIFF_C{}_{}", self.tag, self.consts.len());
            let expr = self.literal(&ty);
            self.consts.push(ConstDef {
                name: name.clone(),
                ty: ty.clone(),
                expr,
            });
            self.scope.push(Binding {
                name,
                ty,
                kind: BindKind::Const,
            });
        }
    }

    pub(super) fn declare_describes(&mut self) {
        let count = self.rng.random_range(0..=2);
        for _ in 0..count {
            let ty = if self.chance(0.7) {
                self.scalar_ty()
            } else {
                Ty::vec_of(self.scalar_ty())
            };
            if !self.describes.contains(&ty) {
                self.describes.push(ty);
            }
        }
    }

    // -- helper functions -----------------------------------------------------

    /// A helper returning `ret`. Answers the name and parameter types.
    pub(super) fn helper_fn(&mut self, ret: &Ty) -> Option<(String, Vec<Param>)> {
        if self.fn_ret.is_some() {
            // No helper inside a helper, or the nesting never ends.
            return None;
        }
        let name = self.fresh("diff_fn");
        let count = self.rng.random_range(0..=2);
        let params: Vec<Param> = (0..count)
            .map(|_| Param {
                name: self.fresh("diff_p"),
                ty: if self.chance(0.7) {
                    self.scalar_ty()
                } else {
                    self.elem_ty()
                },
                mode: if self.chance(0.3) {
                    ParamMode::Ref
                } else {
                    ParamMode::Owned
                },
            })
            .collect();
        let locals: Vec<(String, Ty)> = params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect();
        self.fn_ret = Some(ret.clone());
        let body =
            self.without_scope(|inner| inner.with_locals(&locals, |inner| inner.fn_body(ret)));
        self.fn_ret = None;
        self.fns.push(FnDef {
            name: name.clone(),
            kind: FnKind::Plain {
                params: params.clone(),
                ret: ret.clone(),
                body,
            },
        });
        Some((name, params))
    }

    /// A couple of lets, maybe an early return, and a tail that may be a
    /// bare pipe.
    fn fn_body(&mut self, ret: &Ty) -> Expr {
        let mut stmts = Vec::new();
        let lets = self.rng.random_range(0..=2);
        for _ in 0..lets {
            let ty = self.any_ty();
            let name = self.fresh("diff_l");
            let expr = self.expr(&ty, 2);
            let ann = if expr.states_concrete_ty() && self.chance(0.5) {
                Ann::Inferred
            } else {
                Ann::Typed
            };
            self.push_local(name.clone(), ty.clone());
            stmts.push(Stmt::Let {
                name,
                ty,
                expr,
                ann,
            });
        }
        if self.chance(0.4) {
            stmts.push(self.return_stmt());
        }
        let tail = if self.chance(0.4) {
            self.pipe_collect(ret, crate::lang::pipe::Site::Bare, 2)
                .unwrap_or_else(|| self.expr(ret, 2))
        } else {
            self.expr(ret, 2)
        };
        for _ in 0..lets {
            self.scope.pop();
        }
        if stmts.is_empty() {
            tail
        } else {
            Expr::Block {
                stmts,
                tail: Box::new(tail),
            }
        }
    }

    /// Writes through `&mut T` from the old value and the extra parameters.
    pub(super) fn writer_fn(&mut self, target: &Ty) -> (String, Vec<Ty>) {
        let name = self.fresh("diff_write");
        let count = self.rng.random_range(0..=1);
        let params: Vec<Param> = (0..count)
            .map(|_| Param {
                name: self.fresh("diff_p"),
                ty: self.scalar_ty(),
                mode: ParamMode::Owned,
            })
            .collect();
        let mut locals: Vec<(String, Ty)> = params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect();
        locals.push(("diff_cur".to_string(), target.clone()));
        let value =
            self.without_scope(|inner| inner.with_locals(&locals, |inner| inner.expr(target, 2)));
        self.fns.push(FnDef {
            name: name.clone(),
            kind: FnKind::Writer {
                target: target.clone(),
                params: params.clone(),
                value,
            },
        });
        (name, params.into_iter().map(|param| param.ty).collect())
    }

    pub(super) fn generic_pick_fn(&mut self) -> String {
        if let Some(def) = self
            .fns
            .iter()
            .find(|def| matches!(def.kind, FnKind::GenericPick))
        {
            return def.name.clone();
        }
        let name = format!("diff_pick_{}", self.tag);
        self.fns.push(FnDef {
            name: name.clone(),
            kind: FnKind::GenericPick,
        });
        name
    }

    /// One per type per block.
    pub(super) fn apply_fn(&mut self, ty: &Ty) -> String {
        if let Some(def) = self
            .fns
            .iter()
            .find(|def| matches!(&def.kind, FnKind::Apply { ty: seen } if seen == ty))
        {
            return def.name.clone();
        }
        let name = self.fresh("diff_apply");
        self.fns.push(FnDef {
            name: name.clone(),
            kind: FnKind::Apply { ty: ty.clone() },
        });
        name
    }

    pub(super) fn factory_fn(&mut self, ty: &Ty) -> String {
        let name = self.fresh("diff_factory");
        let op = *self.pick(&[
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::BitXor,
            BinOp::BitOr,
        ]);
        self.fns.push(FnDef {
            name: name.clone(),
            kind: FnKind::Factory { ty: ty.clone(), op },
        });
        name
    }
}
