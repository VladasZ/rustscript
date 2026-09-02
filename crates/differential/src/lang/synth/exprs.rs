//! Expression generation.

use rand::RngExt;

use crate::lang::catalog::{
    ElemReq, FishReq, METHODS, Method, RecvClass, Solved, TyPat, arg_ty, fish_allows, solve,
};
use crate::lang::expr::{BinOp, Expr, unbare_deep};
use crate::lang::pipe::Site;
use crate::lang::synth::{BindKind, Generator};
use crate::lang::ty::{FloatWidth, IntWidth, Ty};

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
                // a repeat of a nested container is where shared rows would show
                if self.chance(0.3) {
                    return Expr::VecRepeat {
                        elem: (**elem).clone(),
                        item: Box::new(self.leaf(elem)),
                        count,
                    };
                }
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

    /// Boundary values first, a width bug shows at the edge of the range.
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
                // a bare negative literal binds looser than a method call
                if value < 0.0 {
                    format!("({value:?}{suffix})")
                } else {
                    format!("{value:?}{suffix}")
                }
            }
        }
    }

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

    /// Full of things that look parseable on purpose, a pool of plain words would never exercise
    /// `parse`.
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

    pub(super) fn argument(
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
    pub(super) fn sample_recv(
        &mut self,
        method: &Method,
        key: Option<&Ty>,
        val: Option<&Ty>,
    ) -> Option<Ty> {
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

    pub(super) fn container_elem(&mut self, method: &Method) -> Option<Ty> {
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

    pub(super) fn sample_fish(&mut self, req: FishReq) -> Option<Ty> {
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
}
