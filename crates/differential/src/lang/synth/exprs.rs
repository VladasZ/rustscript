//! Expression generation.

use rand::RngExt;

use crate::lang::catalog::{
    ElemReq, FishReq, METHODS, Method, RecvClass, Solved, TyPat, arg_ty, fish_allows, solve,
};
use crate::lang::expr::{BinOp, Expr, UnOp, unbare_deep};
use crate::lang::pipe::Site;
use crate::lang::synth::{BindKind, Generator, is_partial_ord};
use crate::lang::ty::{FloatWidth, IntWidth, Ty};
use crate::lang::user::UserShape;

impl Generator<'_> {
    pub fn expr(&mut self, want: &Ty, depth: usize) -> Expr {
        if depth == 0 {
            return self.leaf(want);
        }
        for _ in 0..4 {
            let attempt = match self.rng.random_range(0..100) {
                0..=17 => Some(self.leaf(want)),
                18..=24 => self.pipe_collect(want, Site::Turbofish, depth),
                25..=44 => self.call(want, depth),
                45..=56 => self.binary(want, depth),
                57..=60 => self.cast(want, depth),
                61..=63 => self.unary(want, depth),
                64..=67 => Some(self.branch(want, depth)),
                68..=72 => self.match_expr(want, depth),
                73..=78 => self.access(want, depth),
                79..=85 => self.user_expr(want, depth),
                86..=91 => self.call_named(want, depth),
                92..=95 => self.try_expr(want, depth),
                _ => self.bare_or_const(want),
            };
            if let Some(expr) = attempt {
                return expr;
            }
        }
        self.leaf(want)
    }

    pub(super) fn leaf(&mut self, want: &Ty) -> Expr {
        let matching: Vec<(String, BindKind)> = self
            .scope
            .iter()
            .filter(|binding| {
                binding.ty == *want && matches!(binding.kind, BindKind::Local | BindKind::Const)
            })
            .map(|binding| (binding.name.clone(), binding.kind.clone()))
            .collect();
        if !matching.is_empty() && self.chance(0.5) {
            let (name, kind) = self.pick(&matching).clone();
            return match kind {
                BindKind::Const => Expr::ConstRef {
                    name,
                    ty: want.clone(),
                    opaque: false,
                },
                _ => Expr::Var {
                    name,
                    ty: want.clone(),
                },
            };
        }
        self.literal(want)
    }

    // catalog calls

    pub(super) fn call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        // solving touches no generator state, so it runs first
        let solved: Vec<(&'static Method, Solved)> = METHODS
            .iter()
            .filter_map(|method| Some((method, solve(method, want)?)))
            .collect();
        if solved.is_empty() {
            return None;
        }
        for _ in 0..4 {
            let index = self.rng.random_range(0..solved.len());
            let (method, pinned) = solved[index].clone();
            let Some(recv_ty) = (match pinned.recv {
                Some(ty) => Some(ty),
                None => self.sample_recv(method, pinned.key.as_ref(), pinned.val.as_ref()),
            }) else {
                continue;
            };
            let mut fish = pinned.fish;
            if method.fish != FishReq::None && fish.is_none() {
                fish = self.sample_fish(method.fish);
                if fish.is_none() {
                    continue;
                }
            }
            let recv = unbare_deep(self.typed_only(|inner| inner.expr(&recv_ty, depth - 1)));
            let mut args = Vec::with_capacity(method.args.len());
            let mut usable = true;
            for pattern in method.args {
                if let Some(arg) =
                    self.argument(pattern, method.recv, &recv_ty, fish.as_ref(), depth)
                {
                    args.push(arg);
                } else {
                    usable = false;
                    break;
                }
            }
            if !usable {
                continue;
            }
            return Some(Expr::Call {
                method: method.name.to_string(),
                recv: Box::new(recv),
                args,
                fish,
                ty: want.clone(),
            });
        }
        None
    }

    fn argument(
        &mut self,
        pattern: &TyPat,
        class: RecvClass,
        recv: &Ty,
        fish: Option<&Ty>,
        depth: usize,
    ) -> Option<Expr> {
        // a count stays a small literal, or `repeat` and `pow` eat the whole timeout
        let small = match pattern {
            TyPat::SmallU32 => Some((IntWidth::U32, i128::from(self.rng.random_range(0..=9)))),
            TyPat::SmallI32 => Some((IntWidth::I32, i128::from(self.rng.random_range(-3..=5)))),
            TyPat::SmallUsize => Some((IntWidth::USize, i128::from(self.rng.random_range(0..=5)))),
            _ => None,
        };
        if let Some((width, value)) = small {
            return Some(Expr::IntLit {
                width,
                value,
                opaque: false,
            });
        }
        let ty = arg_ty(pattern, class, recv, fish)?;
        Some(self.expr(&ty, depth - 1))
    }

    /// A receiver type for a method whose result didn't pin one. The sample completes a half
    /// pinned pair.
    fn sample_recv(&mut self, method: &Method, key: Option<&Ty>, val: Option<&Ty>) -> Option<Ty> {
        let ty = match method.recv {
            RecvClass::Int => Ty::Int(self.int_width()),
            RecvClass::SignedInt => {
                let signed: Vec<IntWidth> = crate::lang::ty::INT_WIDTHS
                    .iter()
                    .copied()
                    .filter(|width| width.is_signed())
                    .collect();
                Ty::Int(*self.pick(&signed))
            }
            RecvClass::UnsignedInt => {
                let unsigned: Vec<IntWidth> = crate::lang::ty::INT_WIDTHS
                    .iter()
                    .copied()
                    .filter(|width| !width.is_signed())
                    .collect();
                Ty::Int(*self.pick(&unsigned))
            }
            RecvClass::Float => Ty::Float(self.float_width()),
            RecvClass::Bool => Ty::Bool,
            RecvClass::Char => Ty::Char,
            RecvClass::Str => Ty::Str,
            RecvClass::Vec => Ty::vec_of(self.container_elem(method)?),
            RecvClass::VecOfVec => Ty::vec_of(Ty::vec_of(self.container_elem(method)?)),
            RecvClass::Opt => Ty::opt_of(self.container_elem(method)?),
            RecvClass::Set => Ty::set_of(self.key_ty()),
            RecvClass::Map => {
                let key = match key {
                    Some(key) => key.clone(),
                    None => self.key_ty(),
                };
                let value = match val {
                    Some(value) => value.clone(),
                    None => self.val_ty(),
                };
                Ty::map_of(key, value)
            }
            RecvClass::Res => {
                let ok = match key {
                    Some(ok) => ok.clone(),
                    None => self.scalar_ty(),
                };
                let err = match val {
                    Some(err) => err.clone(),
                    None => match self.res_ty() {
                        Ty::Res(_, err) => *err,
                        _ => Ty::Str,
                    },
                };
                Ty::res_of(ok, err)
            }
        };
        Some(ty)
    }

    fn container_elem(&mut self, method: &Method) -> Option<Ty> {
        for _ in 0..8 {
            let candidate = if self.chance(0.7) {
                self.scalar_ty()
            } else {
                self.elem_ty()
            };
            let allowed = match method.elem {
                ElemReq::Any => true,
                ElemReq::Num => candidate.is_numeric(),
                ElemReq::Ord => candidate.is_ord(),
                ElemReq::Key => candidate.is_key(),
                ElemReq::Default => candidate.has_default(),
                ElemReq::Str => matches!(candidate, Ty::Str),
                ElemReq::Copy => candidate.is_copy(),
            };
            if allowed {
                return Some(candidate);
            }
        }
        (method.elem == ElemReq::Str).then_some(Ty::Str)
    }

    fn sample_fish(&mut self, req: FishReq) -> Option<Ty> {
        for _ in 0..8 {
            let candidate = self.scalar_ty();
            if fish_allows(req, &candidate) {
                return Some(candidate);
            }
        }
        None
    }

    // operators

    pub(super) fn binary(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let (op, right_ty) = match want {
            Ty::Int(_) => {
                let ops = [
                    BinOp::Add,
                    BinOp::Sub,
                    BinOp::Mul,
                    BinOp::Div,
                    BinOp::Rem,
                    BinOp::BitAnd,
                    BinOp::BitOr,
                    BinOp::BitXor,
                    BinOp::Shl,
                    BinOp::Shr,
                ];
                let op = *self.pick(&ops);
                let right = if matches!(op, BinOp::Shl | BinOp::Shr) {
                    Ty::U32
                } else {
                    want.clone()
                };
                (op, right)
            }
            Ty::Float(_) => {
                let ops = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem];
                (*self.pick(&ops), want.clone())
            }
            Ty::Bool => {
                if self.chance(0.4) {
                    let ops = [
                        BinOp::And,
                        BinOp::Or,
                        BinOp::BitXor,
                        BinOp::BitAnd,
                        BinOp::BitOr,
                    ];
                    (*self.pick(&ops), Ty::Bool)
                } else {
                    return Some(self.comparison(depth));
                }
            }
            _ => return None,
        };
        let left = self.expr(want, depth - 1);
        let right = self.expr(&right_ty, depth - 1);
        Some(Expr::Bin {
            op,
            left: Box::new(left),
            right: Box::new(right),
            ty: want.clone(),
        })
    }

    fn comparison(&mut self, depth: usize) -> Expr {
        let operand = if self.chance(0.6) {
            self.scalar_ty()
        } else {
            self.any_ty()
        };
        let ops: &[BinOp] = if is_partial_ord(&operand) {
            &[
                BinOp::Eq,
                BinOp::Ne,
                BinOp::Lt,
                BinOp::Le,
                BinOp::Gt,
                BinOp::Ge,
            ]
        } else {
            &[BinOp::Eq, BinOp::Ne]
        };
        let op = *self.pick(ops);
        let left = self.expr(&operand, depth - 1);
        let right = self.expr(&operand, depth - 1);
        Expr::Bin {
            op,
            left: Box::new(left),
            right: Box::new(right),
            ty: Ty::Bool,
        }
    }

    pub(super) fn cast(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let source = match want {
            Ty::Int(_) | Ty::Float(_) => match self.rng.random_range(0..10) {
                0..=5 => Ty::Int(self.int_width()),
                6 | 7 => Ty::Float(self.float_width()),
                8 if want.is_int() => Ty::Bool,
                _ if want.is_int() => Ty::Char,
                _ => Ty::Int(self.int_width()),
            },
            // only `u8` casts to `char`
            Ty::Char => Ty::Int(IntWidth::U8),
            _ => return None,
        };
        // `char as f64` doesn't exist
        if matches!(source, Ty::Char) && !want.is_int() {
            return None;
        }
        let value = self.expr(&source, depth - 1);
        Some(Expr::Cast {
            value: Box::new(value),
            to: want.clone(),
        })
    }

    pub(super) fn unary(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let op = match want {
            Ty::Int(width) if width.is_signed() => {
                if self.chance(0.5) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                }
            }
            Ty::Int(_) | Ty::Bool => UnOp::Not,
            Ty::Float(_) => UnOp::Neg,
            _ => return None,
        };
        let value = self.expr(want, depth - 1);
        Some(Expr::Unary {
            op,
            value: Box::new(value),
            ty: want.clone(),
        })
    }

    pub(super) fn branch(&mut self, want: &Ty, depth: usize) -> Expr {
        let condition = self.expr(&Ty::Bool, depth - 1);
        let then_expr = self.expr(want, depth - 1);
        let else_expr = self.expr(want, depth - 1);
        Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
            ty: want.clone(),
        }
    }

    /// A bare literal or a const read.
    pub(super) fn bare_or_const(&mut self, want: &Ty) -> Option<Expr> {
        let consts: Vec<String> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Const) && binding.ty == *want)
            .map(|binding| binding.name.clone())
            .collect();
        if !consts.is_empty() && self.chance(0.5) {
            return Some(Expr::ConstRef {
                name: self.pick(&consts).clone(),
                ty: want.clone(),
                opaque: false,
            });
        }
        if self.forbid_bare {
            return None;
        }
        match want {
            Ty::Int(IntWidth::I32) => Some(Expr::BareInt {
                value: self.int_value(IntWidth::I32),
                opaque: false,
            }),
            Ty::Float(FloatWidth::F64) => Some(Expr::BareFloat {
                token: self.bare_float_token(),
                opaque: false,
            }),
            _ => None,
        }
    }

    // accesses

    pub(super) fn access(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<Expr> = Vec::new();
        let locals: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local))
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        for (name, ty) in &locals {
            let base = Expr::Var {
                name: name.clone(),
                ty: ty.clone(),
            };
            match ty {
                Ty::User(shape) => {
                    for (index, field) in shape.fields().iter().enumerate() {
                        if field.ty == *want {
                            options.push(Expr::Field {
                                base: Box::new(base.clone()),
                                index,
                                ty: want.clone(),
                            });
                        }
                    }
                }
                Ty::Tuple(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if item == want {
                            options.push(Expr::TupleField {
                                base: Box::new(base.clone()),
                                index,
                                ty: want.clone(),
                            });
                        }
                    }
                }
                Ty::Vec(elem) if **elem == *want => {
                    let index = if self.chance(0.7) {
                        Expr::IntLit {
                            width: IntWidth::USize,
                            value: i128::from(self.rng.random_range(0..=4u8)),
                            opaque: false,
                        }
                    } else {
                        self.expr(&Ty::USIZE, depth - 1)
                    };
                    options.push(Expr::Index {
                        base: Box::new(base.clone()),
                        index: Box::new(index),
                        ty: want.clone(),
                    });
                }
                _ => {}
            }
        }
        // a field read off a fresh struct value
        if options.is_empty() || self.chance(0.2) {
            let shapes: Vec<UserShape> = self
                .types
                .iter()
                .filter(|def| def.shape.fields().iter().any(|field| field.ty == *want))
                .map(|def| def.shape.clone())
                .collect();
            if let Some(shape) = shapes.first() {
                let shape = shape.clone();
                let index = shape.fields().iter().position(|field| field.ty == *want)?;
                let base = self.expr(&Ty::user(shape), depth - 1);
                return Some(Expr::Field {
                    base: Box::new(base),
                    index,
                    ty: want.clone(),
                });
            }
        }
        if options.is_empty() {
            return None;
        }
        Some(self.pick(&options).clone())
    }
}
