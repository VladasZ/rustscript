//! Expression generation: leaves, literals, operators, casts, branches,
//! catalog calls, accesses into containers and user values, user methods,
//! conversions, and `?`.

use rand::RngExt;

use crate::lang::block::{Param, ParamMode};
use crate::lang::catalog::{
    ElemReq, FishReq, METHODS, Method, RecvClass, Solved, TyPat, arg_ty, fish_allows, solve,
};
use crate::lang::expr::{BinOp, Expr, UnOp, unbare};
use crate::lang::pipe::Site;
use crate::lang::synth::{BindKind, Generator, MAX_EXPR_DEPTH, is_partial_ord};
use crate::lang::ty::{FloatWidth, IntWidth, Ty};
use crate::lang::user::{MethodKind, UserShape};

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

    pub(super) fn literal(&mut self, want: &Ty) -> Expr {
        match want {
            Ty::Int(width) => Expr::IntLit {
                width: *width,
                value: self.int_value(*width),
                opaque: false,
            },
            Ty::Float(width) => Expr::FloatLit {
                width: *width,
                token: self.float_token(*width),
                opaque: false,
            },
            Ty::Bool => Expr::BoolLit {
                value: self.chance(0.5),
                opaque: false,
            },
            Ty::Char => Expr::CharLit {
                value: self.char_value(),
                opaque: false,
            },
            Ty::Str => Expr::StrLit(self.string_value()),
            Ty::Vec(elem) => {
                let count = self.rng.random_range(0..=3);
                let items = (0..count).map(|_| self.leaf(elem)).collect();
                Expr::VecLit {
                    elem: (**elem).clone(),
                    items,
                }
            }
            Ty::Opt(elem) => {
                let value = if self.chance(0.65) {
                    Some(Box::new(self.leaf(elem)))
                } else {
                    None
                };
                Expr::OptLit {
                    elem: (**elem).clone(),
                    value,
                }
            }
            Ty::Map(key, value) => {
                let count = self.rng.random_range(0..=3);
                let items = (0..count)
                    .map(|_| (self.leaf(key), self.leaf(value)))
                    .collect();
                Expr::MapLit {
                    key: (**key).clone(),
                    value: (**value).clone(),
                    items,
                }
            }
            Ty::Set(elem) => {
                let count = self.rng.random_range(0..=3);
                let items = (0..count).map(|_| self.leaf(elem)).collect();
                Expr::SetLit {
                    elem: (**elem).clone(),
                    items,
                }
            }
            Ty::Tuple(items) => Expr::TupleLit(items.iter().map(|item| self.leaf(item)).collect()),
            Ty::Res(ok, err) => {
                let value = if self.chance(0.6) {
                    Ok(Box::new(self.leaf(ok)))
                } else {
                    Err(Box::new(self.leaf(err)))
                };
                Expr::ResLit {
                    ok: (**ok).clone(),
                    err: (**err).clone(),
                    value,
                }
            }
            Ty::StdErr(err) => Expr::StdErrLit(*err),
            Ty::User(shape) => self.user_literal(shape, 0),
        }
    }

    /// Boundary values first. A width bug shows at the edge of the range, not
    /// in the middle of it, so `max`, `max - 1`, `min` and zero are far more
    /// valuable than a uniform draw.
    pub(super) fn int_value(&mut self, width: IntWidth) -> i128 {
        let (min, max) = (width.min(), width.max());
        match self.rng.random_range(0..12) {
            0 => 0,
            1 => 1,
            2 => max,
            3 => max - 1,
            4 => min,
            5 if min != 0 => min + 1,
            6 => max / 2,
            7 => max / 2 + 1,
            8 if min != 0 => -1,
            9 => 2,
            _ => {
                let span = max - min;
                let draw = i128::from(self.rng.random_range(0..=u64::MAX));
                min + draw.rem_euclid(span + 1)
            }
        }
    }

    pub(super) fn float_token(&mut self, width: FloatWidth) -> String {
        let suffix = width.rust();
        match self.rng.random_range(0..12) {
            0 => format!("{suffix}::NAN"),
            1 => format!("{suffix}::INFINITY"),
            2 => format!("{suffix}::NEG_INFINITY"),
            3 => format!("{suffix}::MAX"),
            4 => format!("{suffix}::MIN"),
            5 => format!("{suffix}::EPSILON"),
            6 => format!("0.0{suffix}"),
            7 => format!("(-0.0{suffix})"),
            8 => format!("1.0{suffix}"),
            9 => format!("(-1.0{suffix})"),
            10 => format!("0.5{suffix}"),
            _ => {
                let value = f64::from(self.rng.random_range(0..2_000_000)) / 1000.0 - 1000.0;
                // A bare negative literal would bind looser than a method call
                // on it, so it is parenthesized at the source.
                if value < 0.0 {
                    format!("({value:?}{suffix})")
                } else {
                    format!("{value:?}{suffix}")
                }
            }
        }
    }

    /// A float token with no suffix, for `f64` by inference.
    pub(super) fn bare_float_token(&mut self) -> String {
        match self.rng.random_range(0..6) {
            0 => "0.0".to_string(),
            1 => "1.5".to_string(),
            2 => "(-2.25)".to_string(),
            3 => "1e10".to_string(),
            4 => "0.1".to_string(),
            _ => {
                let value = f64::from(self.rng.random_range(0..200_000)) / 100.0 - 1000.0;
                if value < 0.0 {
                    format!("({value:?})")
                } else {
                    format!("{value:?}")
                }
            }
        }
    }

    pub(super) fn char_value(&mut self) -> char {
        const POOL: &[char] = &[
            'a', 'Z', '0', '9', ' ', '\n', '\t', '_', 'é', 'ß', 'は', '✓', '7', 'f',
        ];
        *self.pick(POOL)
    }

    /// The string pool is deliberately full of things that look parseable.
    /// `parse` is where the target type has to be honored, so a pool of plain
    /// words would never ask the question.
    pub(super) fn string_value(&mut self) -> String {
        const POOL: &[&str] = &[
            "",
            "0",
            "5",
            "-1",
            "300",
            "1.5",
            " 5 ",
            "5 ",
            "+7",
            "007",
            "true",
            "false",
            "TRUE",
            "1",
            "abc",
            "Hello World",
            "  padded  ",
            "a,b,c",
            "99999999999999999999",
            "-99999999999999999999",
            "0x1f",
            "1e3",
            "inf",
            "NaN",
            "é",
            "  ",
            "\n",
            "a\nb\nc",
            "key=value",
            "x,,y",
        ];
        (*self.pick(POOL)).to_string()
    }

    // -- catalog calls --------------------------------------------------------

    /// A catalog method whose result type unifies with the wanted type.
    pub(super) fn call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        // Solving touches no generator state, so it runs first and the random
        // choices that follow borrow `self` on their own.
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
            let recv = unbare(self.typed_only(|inner| inner.expr(&recv_ty, depth - 1)));
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
        // A count argument stays a small literal. A general expression there
        // would let `repeat` or `pow` build something the harness spends its
        // whole timeout on instead of finding a bug.
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

    /// A receiver type for a method whose result did not pin one. A map or
    /// result may have pinned only one half; the sample completes the pair.
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

    // -- operators ------------------------------------------------------------

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

    /// `==` on any type, `<` and friends on anything with an order.
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
            // Only `u8` casts to `char`.
            Ty::Char => Ty::Int(IntWidth::U8),
            _ => return None,
        };
        // `char as f64` does not exist, only integer targets take a char.
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

    /// A bare literal, `i32` or `f64` by rustc's default, or a const read.
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

    // -- accesses -------------------------------------------------------------

    /// A field of a struct binding, a tuple slot, or an index into a vec,
    /// whose type is the wanted one.
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
        // A field read off a fresh struct value, not only off a binding.
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

    // -- user types -----------------------------------------------------------

    /// A struct or enum literal, a `Default`, a user method or associated
    /// function, or a `From` conversion.
    pub(super) fn user_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let method = self.method_call(want, depth);
        if method.is_some() && self.chance(0.5) {
            return method;
        }
        let Ty::User(shape) = want else {
            if want.has_default() && self.chance(0.3) {
                return Some(Expr::DefaultOf(want.clone()));
            }
            return method;
        };
        let shape = shape.clone();
        match self.rng.random_range(0..6) {
            0 if shape.derives.default => Some(Expr::DefaultOf(want.clone())),
            1 if !shape.froms.is_empty() => {
                let src = self.pick(&shape.froms).clone();
                let value = unbare(self.typed_only(|inner| inner.expr(&src, depth - 1)));
                Some(Expr::Into {
                    value: Box::new(value),
                    to: want.clone(),
                    bare: false,
                })
            }
            2 => self.assoc_call(&shape, depth),
            _ => Some(self.user_literal(&shape, depth - 1)),
        }
    }

    /// A literal of a user type with generated field or payload values.
    pub(super) fn user_literal(&mut self, shape: &UserShape, depth: usize) -> Expr {
        if shape.is_enum() {
            let variant = self.rng.random_range(0..shape.variants().len());
            let payload = shape.variants()[variant]
                .payload
                .iter()
                .map(|ty| self.expr(ty, depth))
                .collect();
            return Expr::EnumLit {
                shape: Box::new(shape.clone()),
                variant,
                payload,
            };
        }
        let fields = shape.fields();
        let update = shape.derives.default && fields.len() > 1 && self.chance(0.4);
        let written = if update {
            self.rng.random_range(0..fields.len())
        } else {
            fields.len()
        };
        let values = fields[..written]
            .iter()
            .map(|field| self.expr(&field.ty, depth))
            .collect();
        Expr::StructLit {
            shape: Box::new(shape.clone()),
            fields: values,
            update,
        }
    }

    /// `Type::new(..)` style associated functions returning the type.
    fn assoc_call(&mut self, shape: &UserShape, depth: usize) -> Option<Expr> {
        let assoc: Vec<_> = shape
            .methods
            .iter()
            .filter(|sig| sig.kind == MethodKind::Assoc)
            .cloned()
            .collect();
        if assoc.is_empty() {
            return None;
        }
        let sig = self.pick(&assoc).clone();
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::Method {
            owner: Box::new(shape.clone()),
            name: sig.name,
            kind: MethodKind::Assoc,
            base: None,
            args,
            ty: Ty::user(shape.clone()),
        })
    }

    /// A user method whose return type is the wanted one, on a binding of
    /// that user type or a fresh value of it.
    fn method_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<(UserShape, crate::lang::user::MethodSig)> = Vec::new();
        for def in &self.types {
            let owner = def.ty();
            for sig in &def.shape.methods {
                if sig.kind == MethodKind::Method && sig.ret_ty(&owner) == *want {
                    options.push((def.shape.clone(), sig.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (shape, sig) = self.pick(&options).clone();
        let owner = Ty::user(shape.clone());
        let base = self.expr(&owner, depth - 1);
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::Method {
            owner: Box::new(shape),
            name: sig.name,
            kind: MethodKind::Method,
            base: Some(Box::new(base)),
            args,
            ty: want.clone(),
        })
    }

    // -- named calls ----------------------------------------------------------

    /// A helper function, a closure in scope, the generic pick, an apply
    /// through a closure, or the describe trait.
    pub(super) fn call_named(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<Expr> = Vec::new();
        if let Some(expr) = self.closure_call(want, depth) {
            options.push(expr);
        }
        if let Some(expr) = self.helper_call(want, depth) {
            options.push(expr);
        }
        if *want == Ty::Str
            && let Some(expr) = self.trait_call(depth)
        {
            options.push(expr);
        }
        if self.chance(0.15) {
            let name = self.generic_pick_fn();
            let first = self.expr(want, depth - 1);
            let second = self.expr(want, depth - 1);
            let flag = self.expr(&Ty::Bool, depth - 1);
            options.push(Expr::FnCall {
                name,
                args: vec![first, second, flag],
                by_ref: Vec::new(),
                ty: want.clone(),
            });
        }
        if options.is_empty() {
            return None;
        }
        Some(self.pick(&options).clone())
    }

    fn closure_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let closures: Vec<(String, Vec<Ty>)> = self
            .scope
            .iter()
            .filter_map(|binding| match &binding.kind {
                BindKind::Closure { params, ret } if ret == want => {
                    Some((binding.name.clone(), params.clone()))
                }
                _ => None,
            })
            .collect();
        if closures.is_empty() {
            return None;
        }
        let (name, params) = self.pick(&closures).clone();
        // A one parameter closure over its own return type can also go
        // through the generic apply helper.
        if params.len() == 1 && params[0] == *want && self.chance(0.3) {
            let helper = self.apply_fn(want);
            // The helper holds the closure by `&mut` while the argument is
            // evaluated, so the argument must not call the same closure.
            let arg = self.without_binding(&name, |inner| inner.expr(want, depth - 1));
            return Some(Expr::ApplyCall {
                helper,
                closure: name,
                arg: Box::new(arg),
                ty: want.clone(),
            });
        }
        let args = params.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::ClosureCall {
            name,
            args,
            ty: want.clone(),
        })
    }

    fn helper_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let plain: Vec<(String, Vec<Param>)> = self
            .fns
            .iter()
            .filter_map(|def| match &def.kind {
                crate::lang::block::FnKind::Plain { params, ret, .. } if ret == want => {
                    Some((def.name.clone(), params.clone()))
                }
                _ => None,
            })
            .collect();
        let (name, params) = if plain.is_empty() || self.chance(0.3) {
            // A new helper whose return type is the wanted one, so the
            // wanted type gets a body of its own somewhere out of sight.
            if depth < MAX_EXPR_DEPTH {
                return None;
            }
            self.helper_fn(want)?
        } else {
            self.pick(&plain).clone()
        };
        Some(self.fn_call(name, &params, want, depth - 1))
    }

    /// A call of a helper, arguments generated per parameter.
    pub(super) fn fn_call(
        &mut self,
        name: String,
        params: &[Param],
        ret: &Ty,
        depth: usize,
    ) -> Expr {
        let args = params
            .iter()
            .map(|param| self.expr(&param.ty, depth))
            .collect();
        Expr::FnCall {
            name,
            args,
            by_ref: params
                .iter()
                .map(|param| param.mode == ParamMode::Ref)
                .collect(),
            ty: ret.clone(),
        }
    }

    fn trait_call(&mut self, depth: usize) -> Option<Expr> {
        let mut targets: Vec<Ty> = self.describes.clone();
        targets.extend(
            self.types
                .iter()
                .filter(|def| def.shape.describe)
                .map(crate::lang::user::UserDef::ty),
        );
        if targets.is_empty() {
            return None;
        }
        let ty = self.pick(&targets).clone();
        let base = unbare(self.typed_only(|inner| inner.expr(&ty, depth - 1)));
        Some(Expr::TraitCall {
            base: Box::new(base),
        })
    }

    /// `value?` inside a function whose return type accepts it: an `Option`
    /// body unwraps an `Option`, a `Result` body unwraps a `Result` whose
    /// error type is the function's own or converts into it through `From`.
    pub(super) fn try_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let ret = self.fn_ret.clone()?;
        let inner = match &ret {
            Ty::Opt(_) => Ty::opt_of(want.clone()),
            Ty::Res(_, err) => {
                let mut sources = vec![(**err).clone()];
                if let Ty::User(shape) = &**err {
                    sources.extend(shape.froms.iter().cloned());
                }
                let source = self.pick(&sources).clone();
                Ty::res_of(want.clone(), source)
            }
            _ => return None,
        };
        let value = self.expr(&inner, depth - 1);
        Some(Expr::Try {
            value: Box::new(value),
            ty: want.clone(),
        })
    }
}
