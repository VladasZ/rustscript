//! The type lattice of the inference pass and the unification of its variables.
//!
//! A reference is its referent, the runtime has no references to speak of. A `Box`, `Rc`,
//! `RefCell` or `Mutex` is its content for the same reason. A numeric literal without a suffix is
//! a variable until something fixes it, and an unfixed one ends as `i32` or `f64` like in `rustc`.

use std::sync::Arc;

use crate::interpreter::bytecode::ScalarTy;
use crate::interpreter::numeric::IntWidth;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ty {
    Unknown,
    Unit,
    Bool,
    Char,
    Str,
    Int(IntWidth),
    /// an integer literal nothing has typed yet
    IntVar(u32),
    F32,
    F64,
    /// a float literal nothing has typed yet
    FloatVar(u32),
    Vec(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Tuple(Vec<Ty>),
    /// canonical name of a script struct
    Struct(Arc<str>),
    /// canonical name of a script enum
    Enum(Arc<str>),
    /// an iterator and its item
    Iter(Box<Ty>),
    Range(Box<Ty>),
    Closure(Vec<Ty>, Box<Ty>),
    /// `serde_json::Value`
    Json,
    /// a map entry and its value type
    Entry(Box<Ty>),
    /// a bridge type by its last path segment, with its type arguments
    Named(Arc<str>, Vec<Ty>),
    Generic(Arc<str>),
}

impl Ty {
    pub(crate) fn vec(item: Ty) -> Ty {
        Ty::Vec(Box::new(item))
    }

    pub(crate) fn option(item: Ty) -> Ty {
        Ty::Option(Box::new(item))
    }

    pub(crate) fn result(ok: Ty, err: Ty) -> Ty {
        Ty::Result(Box::new(ok), Box::new(err))
    }

    pub(crate) fn iter(item: Ty) -> Ty {
        Ty::Iter(Box::new(item))
    }

    pub(crate) fn named(name: &str) -> Ty {
        Ty::Named(Arc::from(name), Vec::new())
    }

    pub(crate) fn usize() -> Ty {
        Ty::Int(IntWidth::USize)
    }

    pub(crate) fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub(crate) fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::Int(_) | Ty::IntVar(_) | Ty::F32 | Ty::F64 | Ty::FloatVar(_)
        )
    }

    /// The item a `for` loop or an iterator method sees.
    pub(crate) fn item(&self) -> Ty {
        match self {
            Ty::Vec(t)
            | Ty::Set(t)
            | Ty::Iter(t)
            | Ty::Range(t)
            | Ty::Option(t)
            | Ty::Result(t, _) => (**t).clone(),
            Ty::Map(k, v) => Ty::Tuple(vec![(**k).clone(), (**v).clone()]),
            Ty::Str => Ty::Char,
            _ => Ty::Unknown,
        }
    }

    /// What one unwrap gives.
    pub(crate) fn payload(&self) -> Ty {
        match self {
            Ty::Option(t) | Ty::Result(t, _) => (**t).clone(),
            _ => Ty::Unknown,
        }
    }

    /// The runtime carrier of a type a method needs, `parse::<u8>` and friends.
    pub(crate) fn to_scalar(&self) -> Option<ScalarTy> {
        Some(match self {
            Ty::Int(w) => ScalarTy::Int(*w),
            Ty::F32 => ScalarTy::F32,
            Ty::F64 => ScalarTy::F64,
            Ty::Bool => ScalarTy::Bool,
            Ty::Char => ScalarTy::Char,
            Ty::Str => ScalarTy::Str,
            // a `Result` builds its default on the payload side, like an `Option`
            Ty::Option(t) | Ty::Result(t, _) => {
                ScalarTy::Opt(Box::new(t.to_scalar().unwrap_or(ScalarTy::Other)))
            }
            Ty::Vec(t) => ScalarTy::List(Box::new(t.to_scalar().unwrap_or(ScalarTy::Other))),
            Ty::Map(_, v) => ScalarTy::Map(Box::new(v.to_scalar().unwrap_or(ScalarTy::Other))),
            Ty::Set(t) => ScalarTy::Set(Box::new(t.to_scalar().unwrap_or(ScalarTy::Other))),
            Ty::Unknown | Ty::IntVar(_) | Ty::FloatVar(_) | Ty::Generic(_) => return None,
            _ => ScalarTy::Other,
        })
    }
}

enum Bind<T> {
    Free,
    Fixed(T),
    Link(u32),
}

/// Union find over the literal variables.
pub(crate) struct Vars {
    ints: Vec<Bind<IntWidth>>,
    floats: Vec<Bind<bool>>,
}

impl Vars {
    pub(crate) fn new() -> Vars {
        Vars {
            ints: Vec::new(),
            floats: Vec::new(),
        }
    }

    pub(crate) fn fresh_int(&mut self) -> Ty {
        self.ints.push(Bind::Free);
        Ty::IntVar(u32::try_from(self.ints.len() - 1).expect("var count fits u32"))
    }

    pub(crate) fn fresh_float(&mut self) -> Ty {
        self.floats.push(Bind::Free);
        Ty::FloatVar(u32::try_from(self.floats.len() - 1).expect("var count fits u32"))
    }

    fn int_root(&self, mut id: u32) -> u32 {
        while let Bind::Link(next) = self.ints[id as usize] {
            id = next;
        }
        id
    }

    fn float_root(&self, mut id: u32) -> u32 {
        while let Bind::Link(next) = self.floats[id as usize] {
            id = next;
        }
        id
    }

    fn fix_int(&mut self, id: u32, width: IntWidth) {
        let root = self.int_root(id);
        if let Bind::Free = self.ints[root as usize] {
            self.ints[root as usize] = Bind::Fixed(width);
        }
    }

    fn fix_float(&mut self, id: u32, f32: bool) {
        let root = self.float_root(id);
        if let Bind::Free = self.floats[root as usize] {
            self.floats[root as usize] = Bind::Fixed(f32);
        }
    }

    fn link_int(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.int_root(a), self.int_root(b));
        if ra == rb {
            return;
        }
        match (&self.ints[ra as usize], &self.ints[rb as usize]) {
            (Bind::Fixed(_), _) => self.ints[rb as usize] = Bind::Link(ra),
            _ => self.ints[ra as usize] = Bind::Link(rb),
        }
    }

    fn link_float(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.float_root(a), self.float_root(b));
        if ra == rb {
            return;
        }
        match (&self.floats[ra as usize], &self.floats[rb as usize]) {
            (Bind::Fixed(_), _) => self.floats[rb as usize] = Bind::Link(ra),
            _ => self.floats[ra as usize] = Bind::Link(rb),
        }
    }

    /// Makes `a` and `b` the same type where both describe the same value. A mismatch is left
    /// alone, `rustc` already rejected real ones and the rest is a gap in this pass.
    pub(crate) fn unify(&mut self, a: &Ty, b: &Ty) {
        match (a, b) {
            (Ty::IntVar(x), Ty::IntVar(y)) => self.link_int(*x, *y),
            (Ty::IntVar(x), Ty::Int(w)) | (Ty::Int(w), Ty::IntVar(x)) => self.fix_int(*x, *w),
            (Ty::FloatVar(x), Ty::FloatVar(y)) => self.link_float(*x, *y),
            (Ty::FloatVar(x), Ty::F32) | (Ty::F32, Ty::FloatVar(x)) => self.fix_float(*x, true),
            (Ty::FloatVar(x), Ty::F64) | (Ty::F64, Ty::FloatVar(x)) => self.fix_float(*x, false),
            (Ty::Vec(x), Ty::Vec(y) | Ty::Iter(y))
            | (Ty::Range(x), Ty::Range(y) | Ty::Iter(y))
            | (Ty::Iter(x), Ty::Iter(y) | Ty::Vec(y) | Ty::Range(y))
            | (Ty::Set(x), Ty::Set(y))
            | (Ty::Option(x), Ty::Option(y))
            | (Ty::Entry(x), Ty::Entry(y)) => self.unify(x, y),
            (Ty::Map(k1, v1), Ty::Map(k2, v2)) | (Ty::Result(k1, v1), Ty::Result(k2, v2)) => {
                self.unify(k1, k2);
                self.unify(v1, v2);
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    self.unify(x, y);
                }
            }
            (Ty::Closure(px, rx), Ty::Closure(py, ry)) if px.len() == py.len() => {
                for (x, y) in px.iter().zip(py) {
                    self.unify(x, y);
                }
                self.unify(rx, ry);
            }
            (Ty::Named(n1, a1), Ty::Named(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y);
                }
            }
            _ => {}
        }
    }

    /// The more informative of 2 descriptions of 1 value, unified on the way.
    pub(crate) fn meet(&mut self, a: &Ty, b: &Ty) -> Ty {
        self.unify(a, b);
        match (a, b) {
            (Ty::Unknown | Ty::Generic(_), other) | (other, Ty::Unknown | Ty::Generic(_)) => {
                other.clone()
            }
            (Ty::IntVar(_), fixed @ Ty::Int(_)) | (fixed @ Ty::Int(_), Ty::IntVar(_)) => {
                fixed.clone()
            }
            (Ty::FloatVar(_), fixed @ (Ty::F32 | Ty::F64))
            | (fixed @ (Ty::F32 | Ty::F64), Ty::FloatVar(_)) => fixed.clone(),
            (Ty::Vec(x), Ty::Vec(y)) => Ty::vec(self.meet(x, y)),
            (Ty::Set(x), Ty::Set(y)) => Ty::Set(Box::new(self.meet(x, y))),
            (Ty::Option(x), Ty::Option(y)) => Ty::option(self.meet(x, y)),
            (Ty::Iter(x), Ty::Iter(y)) => Ty::iter(self.meet(x, y)),
            (Ty::Range(x), Ty::Range(y)) => Ty::Range(Box::new(self.meet(x, y))),
            (Ty::Entry(x), Ty::Entry(y)) => Ty::Entry(Box::new(self.meet(x, y))),
            (Ty::Map(k1, v1), Ty::Map(k2, v2)) => {
                Ty::Map(Box::new(self.meet(k1, k2)), Box::new(self.meet(v1, v2)))
            }
            (Ty::Result(k1, v1), Ty::Result(k2, v2)) => {
                Ty::result(self.meet(k1, k2), self.meet(v1, v2))
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                Ty::Tuple(xs.iter().zip(ys).map(|(x, y)| self.meet(x, y)).collect())
            }
            (Ty::Closure(px, rx), Ty::Closure(py, ry)) if px.len() == py.len() => Ty::Closure(
                px.iter().zip(py).map(|(x, y)| self.meet(x, y)).collect(),
                Box::new(self.meet(rx, ry)),
            ),
            (Ty::Named(n1, a1), Ty::Named(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                Ty::Named(
                    n1.clone(),
                    a1.iter().zip(a2).map(|(x, y)| self.meet(x, y)).collect(),
                )
            }
            (a, _) => a.clone(),
        }
    }

    /// Every variable replaced by what it was fixed to, or the `rustc` default.
    pub(crate) fn resolve(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::IntVar(id) => match self.ints[self.int_root(*id) as usize] {
                Bind::Fixed(w) => Ty::Int(w),
                _ => Ty::Int(IntWidth::I32),
            },
            Ty::FloatVar(id) => match self.floats[self.float_root(*id) as usize] {
                Bind::Fixed(true) => Ty::F32,
                _ => Ty::F64,
            },
            Ty::Vec(t) => Ty::vec(self.resolve(t)),
            Ty::Set(t) => Ty::Set(Box::new(self.resolve(t))),
            Ty::Option(t) => Ty::option(self.resolve(t)),
            Ty::Iter(t) => Ty::iter(self.resolve(t)),
            Ty::Range(t) => Ty::Range(Box::new(self.resolve(t))),
            Ty::Entry(t) => Ty::Entry(Box::new(self.resolve(t))),
            Ty::Map(k, v) => Ty::Map(Box::new(self.resolve(k)), Box::new(self.resolve(v))),
            Ty::Result(k, v) => Ty::result(self.resolve(k), self.resolve(v)),
            Ty::Tuple(items) => Ty::Tuple(items.iter().map(|t| self.resolve(t)).collect()),
            Ty::Closure(params, ret) => Ty::Closure(
                params.iter().map(|t| self.resolve(t)).collect(),
                Box::new(self.resolve(ret)),
            ),
            Ty::Named(name, args) => {
                Ty::Named(name.clone(), args.iter().map(|t| self.resolve(t)).collect())
            }
            other => other.clone(),
        }
    }
}
