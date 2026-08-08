//! Type directed generation. The generator is asked for an expression of a
//! type and answers with any shape that produces it: a binding, a literal, an
//! operator, a cast, a branch, or a catalog method whose result unifies with
//! the wanted type.
//!
//! Because every shape is chosen by type rather than by a hand written case,
//! a method added to the catalog immediately appears inside conditions, inside
//! loop bodies, as a receiver of another call, and at any depth.

use std::mem::take;

use rand::RngExt;
use rand::rngs::StdRng;

use crate::lang::catalog::{
    ElemReq, FishReq, METHODS, Method, RecvClass, Solved, TyPat, arg_ty, fish_allows, solve,
};
use crate::lang::expr::{BinOp, Expr, UnOp};
use crate::lang::pipe::{Access, Bind, Item, Pipe, Site, Source, Stage, Term, ty_is_ord};
use crate::lang::stmt::{Block, FnDef, MutOp, Stmt};
use crate::lang::ty::{FLOAT_WIDTHS, INT_WIDTHS, SCALAR_TYPES, Ty, map_combos, set_elems};
use crate::numeric::{FloatWidth, IntWidth};

/// How deep one expression may nest. Enough for a call whose receiver is a
/// call whose argument is an operator, which is where composition bugs live.
const MAX_EXPR_DEPTH: usize = 3;

struct Binding {
    name: String,
    ty: Ty,
}

/// How a collection-typed `let` states its collect target, when a pipe is the
/// initializer at all.
enum CollectRoute {
    Plain,
    BareLet,
    Helper,
}

pub struct Generator<'a> {
    rng: &'a mut StdRng,
    scope: Vec<Binding>,
    labels: usize,
    /// The block's index within its program, baked into helper function names
    /// so two blocks never define the same top-level item.
    tag: usize,
}

impl<'a> Generator<'a> {
    pub fn new(rng: &'a mut StdRng, tag: usize) -> Self {
        Self {
            rng,
            scope: Vec::new(),
            labels: 0,
            tag,
        }
    }

    pub fn block(&mut self) -> Block {
        let mut statements = Vec::new();
        let mut fns = Vec::new();
        let bindings = self.rng.random_range(3..=6);
        for index in 0..bindings {
            let ty = self.any_ty();
            let name = format!("lang_value_{index}");
            // A collection binding takes one of the three collect-target
            // routes: a plain expression (turbofish pipes included), a bare
            // collect whose type only the `let` annotation states, or a call
            // of a helper whose return type states it.
            let expr = match self.collection_route(&ty) {
                CollectRoute::Plain => self.expr(&ty, MAX_EXPR_DEPTH),
                CollectRoute::BareLet => self
                    .pipe_collect(&ty, Site::Bare, MAX_EXPR_DEPTH)
                    .unwrap_or_else(|| self.expr(&ty, MAX_EXPR_DEPTH)),
                CollectRoute::Helper => match self.fn_body(&ty) {
                    Some(body) => {
                        let fn_name = format!("diff_helper_{}_{}", self.tag, self.labels);
                        self.labels += 1;
                        fns.push(FnDef {
                            name: fn_name.clone(),
                            ret: ty.clone(),
                            body,
                        });
                        Expr::FnCall {
                            name: fn_name,
                            ty: ty.clone(),
                        }
                    }
                    None => self.expr(&ty, MAX_EXPR_DEPTH),
                },
            };
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

        let mut block = Block { statements, fns };
        block.seal();
        block
    }

    /// A helper function body. The fn is a top-level item, so the body must
    /// not read main's bindings; scope is emptied while it generates.
    fn fn_body(&mut self, ty: &Ty) -> Option<Expr> {
        let saved = take(&mut self.scope);
        let body = self.pipe_collect(ty, Site::Bare, MAX_EXPR_DEPTH);
        self.scope = saved;
        body
    }

    fn collection_route(&mut self, ty: &Ty) -> CollectRoute {
        if !matches!(ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_)) {
            return CollectRoute::Plain;
        }
        match self.rng.random_range(0..4) {
            0 => CollectRoute::BareLet,
            1 => CollectRoute::Helper,
            _ => CollectRoute::Plain,
        }
    }

    fn next_label(&mut self) -> String {
        let label = format!("lang_{}", self.labels);
        self.labels += 1;
        label
    }

    /// A reassignment, a branch, a loop, an in-place collection mutation, or
    /// an accumulation loop over the existing bindings.
    fn mutation(&mut self) -> Stmt {
        match self.rng.random_range(0..6) {
            0 if !self.scope.is_empty() => {
                let index = self.rng.random_range(0..self.scope.len());
                let name = self.scope[index].name.clone();
                let ty = self.scope[index].ty.clone();
                Stmt::Assign {
                    name,
                    expr: self.expr(&ty, MAX_EXPR_DEPTH - 1),
                }
            }
            4 => match self.collection_mutation() {
                Some(stmt) => stmt,
                None => {
                    let ty = self.any_ty();
                    let expr = self.expr(&ty, MAX_EXPR_DEPTH);
                    let label = self.next_label();
                    Stmt::Print { label, expr }
                }
            },
            5 => match self.accumulation_loop() {
                Some(stmt) => stmt,
                None => {
                    let ty = self.any_ty();
                    let expr = self.expr(&ty, MAX_EXPR_DEPTH);
                    let label = self.next_label();
                    Stmt::Print { label, expr }
                }
            },
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

    /// Generate a mutation-op body with `name` hidden from scope. An
    /// `entry()` chain holds a mutable borrow of the map while its arguments
    /// evaluate, so an argument reading the same binding is a borrow error.
    fn op_without(&mut self, name: &str, build: impl FnOnce(&mut Self) -> MutOp) -> MutOp {
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let op = build(self);
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        op
    }

    /// An in-place mutation of a collection binding in scope.
    fn collection_mutation(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_collection()?;
        let op = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                match self.rng.random_range(0..5) {
                    0 => MutOp::VecPush(self.expr(&elem, 1)),
                    1 if !matches!(elem, Ty::Float(_)) => MutOp::VecSort,
                    2 => MutOp::VecDedup,
                    3 => MutOp::VecSetIndex {
                        index: self.rng.random_range(0..=6),
                        value: self.expr(&elem, 1),
                    },
                    _ => MutOp::VecExtend(self.expr(&Ty::vec_of(elem), 1)),
                }
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                match (&val_ty, self.rng.random_range(0..4)) {
                    (Ty::Int(_), 0) => self.op_without(&name, |inner| MutOp::MapEntryAdd {
                        key: inner.expr(&key_ty, 1),
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    (Ty::Vec(elem), 0) => {
                        let elem = (**elem).clone();
                        self.op_without(&name, |inner| MutOp::MapEntryPush {
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
                if self.rng.random_bool(0.7) {
                    MutOp::SetInsert(self.expr(&elem, 1))
                } else {
                    MutOp::SetRemove(self.expr(&elem, 1))
                }
            }
            _ => return None,
        };
        Some(Stmt::Mutate { name, op })
    }

    /// `for item in vec { accumulate into collection }`: the word-count shape
    /// for maps, plain feeding for vecs and sets. The source is always a vec,
    /// so iteration order is defined.
    fn accumulation_loop(&mut self) -> Option<Stmt> {
        let (target, ty) = self.pick_collection()?;
        let var = format!("diff_item_{}", self.labels);
        self.labels += 1;
        let (source_elem, op) = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::VecPush(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::SetInsert(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                let key_expr = Expr::Var {
                    name: var.clone(),
                    ty: key_ty.clone(),
                };
                let op = match &val_ty {
                    Ty::Int(_) => self.op_without(&target, |inner| MutOp::MapEntryAdd {
                        key: key_expr,
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    Ty::Vec(elem) => {
                        let elem = (**elem).clone();
                        self.op_without(&target, |inner| MutOp::MapEntryPush {
                            key: key_expr,
                            value: inner.expr(&elem, 1),
                        })
                    }
                    _ => MutOp::MapInsert {
                        key: key_expr,
                        value: self.expr(&val_ty, 1),
                    },
                };
                (key_ty, op)
            }
            _ => return None,
        };
        let source = self.expr(&Ty::vec_of(source_elem), 2);
        Some(Stmt::ForAccum {
            var,
            source,
            target,
            op,
        })
    }

    fn pick_collection(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<usize> = self
            .scope
            .iter()
            .enumerate()
            .filter(|(_, binding)| matches!(binding.ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_)))
            .map(|(index, _)| index)
            .collect();
        if matching.is_empty() {
            return None;
        }
        let index = matching[self.rng.random_range(0..matching.len())];
        Some((self.scope[index].name.clone(), self.scope[index].ty.clone()))
    }

    // -- types --------------------------------------------------------------

    fn any_ty(&mut self) -> Ty {
        match self.rng.random_range(0..12) {
            0 => Ty::vec_of(self.scalar_ty()),
            1 => Ty::opt_of(self.scalar_ty()),
            2 => self.map_ty(),
            3 => self.set_ty(),
            _ => self.scalar_ty(),
        }
    }

    fn map_ty(&mut self) -> Ty {
        let combos = map_combos();
        let (key, value) = combos[self.rng.random_range(0..combos.len())].clone();
        Ty::map_of(key, value)
    }

    fn set_ty(&mut self) -> Ty {
        let elems = set_elems();
        Ty::set_of(elems[self.rng.random_range(0..elems.len())].clone())
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
                0..=24 => Some(self.leaf(want)),
                25..=33 => self.pipe_collect(want, Site::Turbofish, depth),
                34..=64 => self.call(want, depth),
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

    /// A receiver type for a method whose result did not pin one. A map
    /// result may have pinned only the key or only the value; the sample
    /// completes the pair within the legal combos.
    fn sample_recv(&mut self, method: &Method, key: Option<&Ty>, val: Option<&Ty>) -> Option<Ty> {
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
            RecvClass::Map => {
                let matching: Vec<(Ty, Ty)> = map_combos()
                    .into_iter()
                    .filter(|(combo_key, combo_val)| {
                        key.is_none_or(|k| k == combo_key) && val.is_none_or(|v| v == combo_val)
                    })
                    .collect();
                if matching.is_empty() {
                    return None;
                }
                let (combo_key, combo_val) =
                    matching[self.rng.random_range(0..matching.len())].clone();
                Ty::map_of(combo_key, combo_val)
            }
            RecvClass::Set => self.set_ty(),
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

    // -- pipelines -----------------------------------------------------------

    /// A pipe whose result is `want`. `site` is where a `collect` states its
    /// target; non-collect terminals ignore it. Every pipe built here must
    /// pass `is_deterministic`, that is asserted rather than assumed.
    fn pipe_collect(&mut self, want: &Ty, site: Site, depth: usize) -> Option<Expr> {
        if depth == 0 {
            return None;
        }
        let pipe = match want {
            Ty::Vec(elem) => self.pipe_to_scalar_collect(elem, want.clone(), site, depth)?,
            Ty::Set(elem) => self.pipe_to_scalar_collect(elem, want.clone(), site, depth)?,
            Ty::Map(key, value) => self.pipe_to_map(key, value, site, depth)?,
            Ty::Int(_) => self.pipe_to_int(want, depth)?,
            Ty::Bool => self.pipe_any(depth)?,
            Ty::Opt(inner) => self.pipe_to_opt(inner, depth)?,
            _ => return None,
        };
        assert!(
            pipe.is_deterministic(),
            "generated a nondeterministic pipe: {}",
            pipe.render()
        );
        Some(Expr::Pipe(Box::new(pipe)))
    }

    /// A source of scalar items, with whether its order is defined.
    fn scalar_source(&mut self, depth: usize) -> (Source, bool) {
        match self.rng.random_range(0..6) {
            0 => {
                let start = self.rng.random_range(-20..=20);
                let count = self.rng.random_range(0..=6);
                (Source::Range { start, count }, true)
            }
            1 => {
                let set = self.set_ty();
                let expr = self.expr(&set, depth - 1);
                (
                    Source::Coll {
                        expr,
                        access: Access::SetInto,
                    },
                    false,
                )
            }
            2 => {
                let map = self.map_ty();
                let expr = self.expr(&map, depth - 1);
                let access = if self.rng.random_bool(0.5) {
                    Access::MapKeys
                } else {
                    Access::MapValues
                };
                (Source::Coll { expr, access }, false)
            }
            _ => {
                let elem = self.scalar_ty();
                let expr = self.expr(&Ty::vec_of(elem), depth - 1);
                (
                    Source::Coll {
                        expr,
                        access: Access::VecInto,
                    },
                    true,
                )
            }
        }
    }

    fn fresh_bind(&mut self) -> String {
        let name = format!("diff_x_{}", self.labels);
        self.labels += 1;
        name
    }

    /// Generate `body` with `bind: ty` visible as a variable.
    fn body_with(&mut self, bind: &str, bind_ty: &Ty, want: &Ty, depth: usize) -> Expr {
        self.scope.push(Binding {
            name: bind.to_string(),
            ty: bind_ty.clone(),
        });
        let body = self.expr(want, depth);
        self.scope.pop();
        body
    }

    /// Scalar items collected into a vec or set of `elem`.
    fn pipe_to_scalar_collect(
        &mut self,
        elem: &Ty,
        target: Ty,
        site: Site,
        depth: usize,
    ) -> Option<Pipe> {
        let (source, ordered) = self.scalar_source(depth);
        let mut stages = Vec::new();
        let mut item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        if item != *elem || self.rng.random_bool(0.3) {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, elem, depth - 1);
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
            });
            item = elem.clone();
        }
        if self.rng.random_bool(0.4) {
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
            stages.push(Stage::Filter {
                bind: Bind::One(bind),
                pred,
            });
        }
        // A vec keeps arrival order, so an unordered source must sort first.
        // A set forgets it, no sort needed.
        if matches!(target, Ty::Vec(_)) {
            if !ordered {
                if !ty_is_ord(&item) {
                    return None;
                }
                stages.push(Stage::Sorted);
            }
            if self.rng.random_bool(0.3) {
                stages.push(match self.rng.random_range(0..3) {
                    0 => Stage::Rev,
                    1 => Stage::Take(self.rng.random_range(0..=5)),
                    _ => Stage::Skip(self.rng.random_range(0..=3)),
                });
            }
        }
        // A set element must hash; keep it inside the generated element pool.
        if matches!(target, Ty::Set(_)) && !set_elems().contains(&item) {
            return None;
        }
        Some(Pipe {
            source,
            stages,
            term: Term::Collect { target, site },
        })
    }

    /// Pair items collected into a map. Three roads to a pair: a map source
    /// iterated whole, a scalar source paired with a computed value, or an
    /// ordered scalar source enumerated when the key is i64.
    fn pipe_to_map(&mut self, key: &Ty, value: &Ty, site: Site, depth: usize) -> Option<Pipe> {
        let target = Ty::map_of(key.clone(), value.clone());
        let choice = self.rng.random_range(0..3);
        if choice == 0 {
            let expr = self.expr(&target, depth - 1);
            let mut stages = Vec::new();
            if self.rng.random_bool(0.4) {
                let key_bind = self.fresh_bind();
                let val_bind = self.fresh_bind();
                self.scope.push(Binding {
                    name: key_bind.clone(),
                    ty: key.clone(),
                });
                self.scope.push(Binding {
                    name: val_bind.clone(),
                    ty: value.clone(),
                });
                let pred = self.expr(&Ty::Bool, depth - 1);
                self.scope.pop();
                self.scope.pop();
                stages.push(Stage::Filter {
                    bind: Bind::Pair(key_bind, val_bind),
                    pred,
                });
            }
            return Some(Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::MapPairs,
                },
                stages,
                term: Term::Collect { target, site },
            });
        }
        if choice == 1 && *key == Ty::I64 {
            // Enumerate is order sensitive, so the source must be a vec.
            let expr = self.expr(&Ty::vec_of(value.clone()), depth - 1);
            return Some(Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::VecInto,
                },
                stages: vec![Stage::Enumerate],
                term: Term::Collect { target, site },
            });
        }
        let expr = self.expr(&Ty::vec_of(key.clone()), depth - 1);
        let bind = self.fresh_bind();
        let body = self.body_with(&bind, key, value, depth - 1);
        Some(Pipe {
            source: Source::Coll {
                expr,
                access: Access::VecInto,
            },
            stages: vec![Stage::PairWith {
                bind: Bind::One(bind),
                body,
            }],
            term: Term::Collect { target, site },
        })
    }

    /// `sum`, `count`, or `fold` down to an integer.
    fn pipe_to_int(&mut self, want: &Ty, depth: usize) -> Option<Pipe> {
        let (source, ordered) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        if *want == Ty::USIZE && self.rng.random_bool(0.4) {
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
            return Some(Pipe {
                source,
                stages: vec![Stage::Filter {
                    bind: Bind::One(bind),
                    pred,
                }],
                term: Term::Count,
            });
        }
        let mut stages = Vec::new();
        let mut item = item;
        if item != *want {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, want, depth - 1);
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
            });
            item = want.clone();
        }
        if ordered && self.rng.random_bool(0.3) {
            let acc = self.fresh_bind();
            let bind = self.fresh_bind();
            let init = self.expr(want, depth - 1);
            self.scope.push(Binding {
                name: acc.clone(),
                ty: want.clone(),
            });
            self.scope.push(Binding {
                name: bind.clone(),
                ty: item,
            });
            let body = self.expr(want, depth - 1);
            self.scope.pop();
            self.scope.pop();
            return Some(Pipe {
                source,
                stages,
                term: Term::Fold {
                    acc,
                    bind: Bind::One(bind),
                    init,
                    body,
                },
            });
        }
        Some(Pipe {
            source,
            stages,
            term: Term::Sum { out: want.clone() },
        })
    }

    fn pipe_any(&mut self, depth: usize) -> Option<Pipe> {
        let (source, _) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        let bind = self.fresh_bind();
        let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
        Some(Pipe {
            source,
            stages: Vec::new(),
            term: Term::Any {
                bind: Bind::One(bind),
                pred,
            },
        })
    }

    /// `min`, `max`, or an ordered `position` into an Option.
    fn pipe_to_opt(&mut self, inner: &Ty, depth: usize) -> Option<Pipe> {
        if *inner == Ty::USIZE && self.rng.random_bool(0.4) {
            let elem = self.scalar_ty();
            let expr = self.expr(&Ty::vec_of(elem.clone()), depth - 1);
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &elem, &Ty::Bool, depth - 1);
            return Some(Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::VecInto,
                },
                stages: Vec::new(),
                term: Term::Position {
                    bind: Bind::One(bind),
                    pred,
                },
            });
        }
        if !ty_is_ord(inner) {
            return None;
        }
        let (source, _) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        let mut stages = Vec::new();
        if item != *inner {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, inner, depth - 1);
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
            });
        }
        let term = if self.rng.random_bool(0.5) {
            Term::Min
        } else {
            Term::Max
        };
        Some(Pipe {
            source,
            stages,
            term,
        })
    }
}
