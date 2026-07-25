//! Type directed generation. The generator is asked for an expression of a
//! type and answers with any shape that produces it: a binding, a literal, an
//! operator, a cast, a branch, or a catalog method whose result unifies with
//! the wanted type.
//!
//! Because every shape is chosen by type rather than by a hand written case,
//! a method added to the catalog immediately appears inside conditions, inside
//! loop bodies, as a receiver of another call, and at any depth.

use rand::RngExt;
use rand::rngs::StdRng;

use crate::lang::catalog::{
    ElemReq, FishReq, METHODS, Method, RecvClass, TyPat, arg_ty, fish_allows, solve,
};
use crate::lang::expr::{BinOp, Expr, UnOp};
use crate::lang::stmt::{Block, Stmt};
use crate::lang::ty::{FLOAT_WIDTHS, INT_WIDTHS, SCALAR_TYPES, Ty};
use crate::numeric::{FloatWidth, IntWidth};

/// How deep one expression may nest. Enough for a call whose receiver is a
/// call whose argument is an operator, which is where composition bugs live.
const MAX_EXPR_DEPTH: usize = 3;

struct Binding {
    name: String,
    ty: Ty,
}

pub struct Generator<'a> {
    rng: &'a mut StdRng,
    scope: Vec<Binding>,
    labels: usize,
}

impl<'a> Generator<'a> {
    pub fn new(rng: &'a mut StdRng) -> Self {
        Self {
            rng,
            scope: Vec::new(),
            labels: 0,
        }
    }

    pub fn block(&mut self) -> Block {
        let mut statements = Vec::new();
        let bindings = self.rng.random_range(3..=6);
        for index in 0..bindings {
            let ty = self.any_ty();
            let name = format!("lang_value_{index}");
            let expr = self.expr(&ty, MAX_EXPR_DEPTH);
            statements.push(Stmt::Let {
                name: name.clone(),
                ty: ty.clone(),
                expr,
            });
            self.scope.push(Binding { name, ty });
        }

        let extras = self.rng.random_range(2..=5);
        for _ in 0..extras {
            statements.push(self.mutation());
        }

        // Every binding is observed, then a few free standing expressions, so
        // a divergence in a value that was never stored still shows up.
        let observed: Vec<Expr> = self
            .scope
            .iter()
            .map(|binding| Expr::Var {
                name: binding.name.clone(),
                ty: binding.ty.clone(),
            })
            .collect();
        for expr in observed {
            let label = self.next_label();
            statements.push(Stmt::Print { label, expr });
        }
        let observations = self.rng.random_range(2..=5);
        for _ in 0..observations {
            let ty = self.any_ty();
            let expr = self.expr(&ty, MAX_EXPR_DEPTH);
            let label = self.next_label();
            statements.push(Stmt::Print { label, expr });
        }

        let mut block = Block { statements };
        block.seal();
        block
    }

    fn next_label(&mut self) -> String {
        let label = format!("lang_{}", self.labels);
        self.labels += 1;
        label
    }

    /// A reassignment, a branch, or a loop over the existing bindings.
    fn mutation(&mut self) -> Stmt {
        match self.rng.random_range(0..4) {
            0 if !self.scope.is_empty() => {
                let index = self.rng.random_range(0..self.scope.len());
                let name = self.scope[index].name.clone();
                let ty = self.scope[index].ty.clone();
                Stmt::Assign {
                    name,
                    expr: self.expr(&ty, MAX_EXPR_DEPTH - 1),
                }
            }
            1 => {
                let condition = self.expr(&Ty::Bool, 2);
                let then_body = self.nested_body();
                let else_body = self.nested_body();
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            2 => {
                let count = self.rng.random_range(0..=3);
                let body = self.nested_body();
                // The counter is never read, so it binds to `_` rather than a
                // name that would make every generated program warn.
                Stmt::ForRange {
                    var: "_".to_string(),
                    count,
                    body,
                }
            }
            _ => {
                let ty = self.any_ty();
                let expr = self.expr(&ty, MAX_EXPR_DEPTH);
                let label = self.next_label();
                Stmt::Print { label, expr }
            }
        }
    }

    /// A nested block writes to existing bindings and prints, but declares
    /// nothing, so every name stays live for the whole program and the reducer
    /// never has to reason about shadowing.
    fn nested_body(&mut self) -> Vec<Stmt> {
        let count = self.rng.random_range(1..=2);
        let mut body = Vec::new();
        for _ in 0..count {
            if !self.scope.is_empty() && self.rng.random_bool(0.5) {
                let index = self.rng.random_range(0..self.scope.len());
                let name = self.scope[index].name.clone();
                let ty = self.scope[index].ty.clone();
                body.push(Stmt::Assign {
                    name,
                    expr: self.expr(&ty, 2),
                });
            } else {
                let ty = self.any_ty();
                let expr = self.expr(&ty, 2);
                let label = self.next_label();
                body.push(Stmt::Print { label, expr });
            }
        }
        body
    }

    // -- types --------------------------------------------------------------

    fn any_ty(&mut self) -> Ty {
        match self.rng.random_range(0..10) {
            0 => Ty::vec_of(self.scalar_ty()),
            1 => Ty::opt_of(self.scalar_ty()),
            _ => self.scalar_ty(),
        }
    }

    fn scalar_ty(&mut self) -> Ty {
        let index = self.rng.random_range(0..SCALAR_TYPES.len());
        SCALAR_TYPES[index].clone()
    }

    fn int_width(&mut self) -> IntWidth {
        INT_WIDTHS[self.rng.random_range(0..INT_WIDTHS.len())]
    }

    fn float_width(&mut self) -> FloatWidth {
        FLOAT_WIDTHS[self.rng.random_range(0..FLOAT_WIDTHS.len())]
    }

    // -- expressions --------------------------------------------------------

    pub fn expr(&mut self, want: &Ty, depth: usize) -> Expr {
        if depth == 0 {
            return self.leaf(want);
        }
        for _ in 0..3 {
            let attempt = match self.rng.random_range(0..100) {
                0..=29 => Some(self.leaf(want)),
                30..=64 => self.call(want, depth),
                65..=82 => self.binary(want, depth),
                83..=88 => self.cast(want, depth),
                89..=93 => self.unary(want, depth),
                _ => self.branch(want, depth),
            };
            if let Some(expr) = attempt {
                return expr;
            }
        }
        self.leaf(want)
    }

    fn leaf(&mut self, want: &Ty) -> Expr {
        let matching: Vec<usize> = self
            .scope
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.ty == *want)
            .map(|(index, _)| index)
            .collect();
        if !matching.is_empty() && self.rng.random_bool(0.5) {
            let index = matching[self.rng.random_range(0..matching.len())];
            return Expr::Var {
                name: self.scope[index].name.clone(),
                ty: want.clone(),
            };
        }
        self.literal(want)
    }

    fn literal(&mut self, want: &Ty) -> Expr {
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
            Ty::Bool => Expr::BoolLit(self.rng.random_bool(0.5)),
            Ty::Char => Expr::CharLit(self.char_value()),
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
                let value = if self.rng.random_bool(0.65) {
                    Some(Box::new(self.leaf(elem)))
                } else {
                    None
                };
                Expr::OptLit {
                    elem: (**elem).clone(),
                    value,
                }
            }
        }
    }

    /// Boundary values first. A width bug shows at the edge of the range, not
    /// in the middle of it, so `max`, `max - 1`, `min` and zero are far more
    /// valuable than a uniform draw.
    fn int_value(&mut self, width: IntWidth) -> i128 {
        let (min, max) = (width.min(), width.max());
        match self.rng.random_range(0..10) {
            0 => 0,
            1 => 1,
            2 => max,
            3 => max - 1,
            4 => min,
            5 if min != 0 => min + 1,
            6 => max / 2,
            _ => {
                let span = max - min;
                let draw = i128::from(self.rng.random_range(0..=u64::MAX));
                min + draw.rem_euclid(span + 1)
            }
        }
    }

    fn float_token(&mut self, width: FloatWidth) -> String {
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

    fn char_value(&mut self) -> char {
        const POOL: &[char] = &[
            'a', 'Z', '0', '9', ' ', '\n', '\t', '_', 'é', 'ß', 'は', '✓',
        ];
        POOL[self.rng.random_range(0..POOL.len())]
    }

    /// The string pool is deliberately full of things that look parseable.
    /// `parse` is where the target type has to be honored, so a pool of plain
    /// words would never ask the question.
    fn string_value(&mut self) -> String {
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
        ];
        POOL[self.rng.random_range(0..POOL.len())].to_string()
    }

    /// A catalog method whose result type unifies with the wanted type.
    fn call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        // Solving touches no generator state, so it runs first and the random
        // choices that follow borrow `self` on their own.
        let solved: Vec<(&'static Method, Option<Ty>, Option<Ty>)> = METHODS
            .iter()
            .filter_map(|method| {
                let solved = solve(method, want)?;
                Some((method, solved.recv, solved.fish))
            })
            .collect();
        if solved.is_empty() {
            return None;
        }
        for _ in 0..4 {
            let index = self.rng.random_range(0..solved.len());
            let (method, pinned_recv, pinned_fish) = solved[index].clone();
            let Some(recv_ty) = (match pinned_recv {
                Some(ty) => Some(ty),
                None => self.sample_recv(method),
            }) else {
                continue;
            };
            let mut fish = pinned_fish;
            if method.fish != FishReq::None && fish.is_none() {
                fish = self.sample_fish(method.fish);
                if fish.is_none() {
                    continue;
                }
            }
            let recv = self.expr(&recv_ty, depth - 1);
            let mut args = Vec::with_capacity(method.args.len());
            let mut usable = true;
            for pattern in method.args {
                match self.argument(pattern, &recv_ty, fish.as_ref(), depth) {
                    Some(arg) => args.push(arg),
                    None => {
                        usable = false;
                        break;
                    }
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
        recv: &Ty,
        fish: Option<&Ty>,
        depth: usize,
    ) -> Option<Expr> {
        // A count argument stays a small literal. A general expression there
        // would let `repeat` or `pow` build something the harness spends its
        // whole timeout on instead of finding a bug.
        let small = match pattern {
            TyPat::SmallU32 => Some((IntWidth::U32, self.rng.random_range(0..=9) as i128)),
            TyPat::SmallI32 => Some((IntWidth::I32, self.rng.random_range(-3..=5) as i128)),
            TyPat::SmallUsize => Some((IntWidth::USize, self.rng.random_range(0..=5) as i128)),
            _ => None,
        };
        if let Some((width, value)) = small {
            return Some(Expr::IntLit {
                width,
                value,
                opaque: false,
            });
        }
        let ty = arg_ty(pattern, recv, fish)?;
        Some(self.expr(&ty, depth - 1))
    }

    /// A receiver type for a method whose result did not pin one.
    fn sample_recv(&mut self, method: &Method) -> Option<Ty> {
        let ty = match method.recv {
            RecvClass::Int => Ty::Int(self.int_width()),
            RecvClass::SignedInt => {
                let signed: Vec<IntWidth> = INT_WIDTHS
                    .iter()
                    .copied()
                    .filter(|width| width.is_signed())
                    .collect();
                Ty::Int(signed[self.rng.random_range(0..signed.len())])
            }
            RecvClass::UnsignedInt => {
                let unsigned: Vec<IntWidth> = INT_WIDTHS
                    .iter()
                    .copied()
                    .filter(|width| !width.is_signed())
                    .collect();
                Ty::Int(unsigned[self.rng.random_range(0..unsigned.len())])
            }
            RecvClass::Float => Ty::Float(self.float_width()),
            RecvClass::Bool => Ty::Bool,
            RecvClass::Char => Ty::Char,
            RecvClass::Str => Ty::Str,
            RecvClass::Vec => Ty::vec_of(self.container_elem(method)?),
            RecvClass::Opt => Ty::opt_of(self.container_elem(method)?),
        };
        Some(ty)
    }

    fn container_elem(&mut self, method: &Method) -> Option<Ty> {
        for _ in 0..8 {
            let candidate = self.scalar_ty();
            let allowed = match method.elem {
                ElemReq::Any => true,
                ElemReq::Num => candidate.is_numeric(),
                ElemReq::Ord => !matches!(candidate, Ty::Float(_)),
            };
            if allowed {
                return Some(candidate);
            }
        }
        None
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

    fn binary(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
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
                let op = ops[self.rng.random_range(0..ops.len())];
                let right = if matches!(op, BinOp::Shl | BinOp::Shr) {
                    Ty::U32
                } else {
                    want.clone()
                };
                (op, right)
            }
            Ty::Float(_) => {
                let ops = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div];
                (ops[self.rng.random_range(0..ops.len())], want.clone())
            }
            Ty::Bool => {
                if self.rng.random_bool(0.5) {
                    let ops = [BinOp::And, BinOp::Or];
                    (ops[self.rng.random_range(0..ops.len())], Ty::Bool)
                } else {
                    let ops = [
                        BinOp::Eq,
                        BinOp::Ne,
                        BinOp::Lt,
                        BinOp::Le,
                        BinOp::Gt,
                        BinOp::Ge,
                    ];
                    let op = ops[self.rng.random_range(0..ops.len())];
                    let operand = self.scalar_ty();
                    let left = self.expr(&operand, depth - 1);
                    let right = self.expr(&operand, depth - 1);
                    return Some(Expr::Bin {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                        ty: Ty::Bool,
                    });
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

    fn cast(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        if !want.is_numeric() {
            return None;
        }
        let source = if self.rng.random_bool(0.7) {
            Ty::Int(self.int_width())
        } else {
            Ty::Float(self.float_width())
        };
        let value = self.expr(&source, depth - 1);
        Some(Expr::Cast {
            value: Box::new(value),
            to: want.clone(),
        })
    }

    fn unary(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let op = match want {
            Ty::Int(width) if width.is_signed() => {
                if self.rng.random_bool(0.5) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                }
            }
            Ty::Int(_) => UnOp::Not,
            Ty::Float(_) => UnOp::Neg,
            Ty::Bool => UnOp::Not,
            _ => return None,
        };
        let value = self.expr(want, depth - 1);
        Some(Expr::Unary {
            op,
            value: Box::new(value),
            ty: want.clone(),
        })
    }

    fn branch(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let condition = self.expr(&Ty::Bool, depth - 1);
        let then_expr = self.expr(want, depth - 1);
        let else_expr = self.expr(want, depth - 1);
        Some(Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
            ty: want.clone(),
        })
    }
}
