//! The typed method catalog. Every method is 1 row, a receiver class, argument patterns, a result
//! pattern and a template. A new row composes at any depth at once. The `surface` command
//! measures the gap against `std_surface.txt`.

use std::sync::LazyLock;

use crate::lang::ty::{FloatWidth, IntWidth, Ty};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecvClass {
    Int,
    SignedInt,
    UnsignedInt,
    Float,
    Bool,
    Char,
    Str,
    Vec,
    /// `Vec<Vec<E>>`, for `concat` and friends
    VecOfVec,
    Opt,
    Res,
    Map,
    Set,
}

impl RecvClass {
    pub fn accepts(self, ty: &Ty) -> bool {
        match self {
            Self::Int => ty.is_int(),
            Self::SignedInt => matches!(ty, Ty::Int(width) if width.is_signed()),
            Self::UnsignedInt => matches!(ty, Ty::Int(width) if !width.is_signed()),
            Self::Float => matches!(ty, Ty::Float(_)),
            Self::Bool => matches!(ty, Ty::Bool),
            Self::Char => matches!(ty, Ty::Char),
            Self::Str => matches!(ty, Ty::Str),
            Self::Vec => matches!(ty, Ty::Vec(_)),
            Self::VecOfVec => matches!(ty, Ty::Vec(inner) if matches!(**inner, Ty::Vec(_))),
            Self::Opt => matches!(ty, Ty::Opt(_)),
            Self::Res => matches!(ty, Ty::Res(..)),
            Self::Map => matches!(ty, Ty::Map(..)),
            Self::Set => matches!(ty, Ty::Set(_)),
        }
    }

    /// Whether an `Elem` result can name the receiver as a container over the wanted type. `Map`
    /// and `Res` need 2 types and are completed in `solve`.
    pub fn is_container(self) -> bool {
        matches!(self, Self::Vec | Self::VecOfVec | Self::Opt | Self::Set)
    }

    pub fn wrap(self, elem: Ty) -> Option<Ty> {
        match self {
            Self::Vec => Some(Ty::vec_of(elem)),
            Self::VecOfVec => Some(Ty::vec_of(Ty::vec_of(elem))),
            Self::Opt => Some(Ty::opt_of(elem)),
            Self::Set => Some(Ty::set_of(elem)),
            _ => None,
        }
    }
}

/// A type in a signature, relative to the receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TyPat {
    Same,
    /// the element type, or the inner element of a `Vec<Vec<E>>`
    Elem,
    Key,
    Val,
    OkT,
    ErrT,
    Exact(Fixed),
    Vec(&'static TyPat),
    Opt(&'static TyPat),
    Tuple2(&'static TyPat, &'static TyPat),
    Res(&'static TyPat, &'static TyPat),
    /// a turbofish type the generator picks
    Fish,
    /// a small literal, so `repeat` and `pow` can't blow up runtime
    SmallU32,
    SmallI32,
    SmallUsize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fixed {
    Bool,
    Char,
    Str,
    U32,
    U64,
    USize,
    F64,
}

impl Fixed {
    pub fn ty(self) -> Ty {
        match self {
            Self::Bool => Ty::Bool,
            Self::Char => Ty::Char,
            Self::Str => Ty::Str,
            Self::U32 => Ty::Int(IntWidth::U32),
            Self::U64 => Ty::Int(IntWidth::U64),
            Self::USize => Ty::Int(IntWidth::USize),
            Self::F64 => Ty::Float(FloatWidth::F64),
        }
    }
}

/// A constraint on the element type, so the call compiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElemReq {
    Any,
    /// `Ord`, every generated type except the floats
    Ord,
    /// an integer or a float, for `sum` and `product`
    Num,
    /// hashable, `Eq`, and ordered, for a set or a map key
    Key,
    /// `Default`, for `unwrap_or_default`
    Default,
    /// exactly `String`, for `join` and `concat`
    Str,
    /// `Copy`, for slice `repeat`
    Copy,
}

impl ElemReq {
    fn allows(self, ty: &Ty) -> bool {
        match self {
            Self::Any => true,
            Self::Ord => ty.is_ord(),
            Self::Num => ty.is_numeric(),
            Self::Key => ty.is_key(),
            Self::Default => ty.has_default(),
            Self::Str => matches!(ty, Ty::Str),
            Self::Copy => ty.is_copy(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FishReq {
    None,
    /// any `FromStr` type
    ParseTarget,
    /// any scalar, for `then_some`
    Scalar,
}

#[derive(Clone, Copy)]
pub struct Method {
    pub name: &'static str,
    pub recv: RecvClass,
    pub args: &'static [TyPat],
    pub ret: TyPat,
    pub elem: ElemReq,
    pub fish: FishReq,
    /// `{r}` receiver, `{0}`.. arguments, `{E}` element type, `{T}` turbofish, `{K}` and `{V}`
    /// the key and value or the ok and error types
    pub template: &'static str,
}

const fn m(
    name: &'static str,
    recv: RecvClass,
    args: &'static [TyPat],
    ret: TyPat,
    template: &'static str,
) -> Method {
    Method {
        name,
        recv,
        args,
        ret,
        elem: ElemReq::Any,
        fish: FishReq::None,
        template,
    }
}

const fn with_elem(method: Method, elem: ElemReq) -> Method {
    Method { elem, ..method }
}

const fn with_fish(method: Method, fish: FishReq) -> Method {
    Method { fish, ..method }
}

use ElemReq::{Default as DefaultElem, Key as KeyElem, Num, Ord as OrdElem, Str as StrElem};
use Fixed::{
    Bool as FBool, Char as FChar, F64 as FF64, Str as FStr, U32 as FU32, U64 as FU64,
    USize as FUSize,
};
use RecvClass::{
    Char, Float, Int, Opt, Res, SignedInt, Str, UnsignedInt, Vec as VecRecv, VecOfVec,
};
use TyPat::{Elem, ErrT, Exact, Fish, OkT, Same, SmallI32, SmallU32, SmallUsize};

const SAME: &TyPat = &Same;
const ELEM: &TyPat = &Elem;
const OK_PAT: &TyPat = &OkT;
const ERR_PAT: &TyPat = &ErrT;
const KEY_PAT: &TyPat = &TyPat::Key;
const VAL_PAT: &TyPat = &TyPat::Val;
const USIZE_PAT: &TyPat = &Exact(FUSize);
const U32_PAT: &TyPat = &Exact(FU32);
const STR_PAT: &TyPat = &Exact(FStr);
const BOOL_PAT: &TyPat = &Exact(FBool);
const CHAR_PAT: &TyPat = &Exact(FChar);

pub static METHODS: LazyLock<Vec<Method>> = LazyLock::new(|| {
    [
        rows_num::ROWS,
        rows_text::ROWS,
        rows_vec::ROWS,
        rows_containers::ROWS,
    ]
    .concat()
});

mod rows_containers;
mod rows_num;
mod rows_text;
mod rows_vec;

#[derive(Clone, Debug)]
pub struct Solved {
    /// `None` means any type in the receiver class works and the generator picks, guided by `key`
    /// or `val` when half a pair is pinned.
    pub recv: Option<Ty>,
    pub fish: Option<Ty>,
    pub key: Option<Ty>,
    pub val: Option<Ty>,
}

/// `None` when this method can never produce the wanted type.
pub fn solve(method: &Method, want: &Ty) -> Option<Solved> {
    let mut found = Found::default();
    unify(&method.ret, want, &mut found)?;
    if found.same.is_none() && matches!(method.recv, RecvClass::Map | RecvClass::Res) {
        return solve_pair(method, found);
    }
    if found.key.is_some() || found.val.is_some() {
        return None;
    }
    let recv = match (found.same, found.elem) {
        (Some(same), _) => {
            if !method.recv.accepts(&same) {
                return None;
            }
            Some(same)
        }
        (None, Some(elem)) => {
            if !method.recv.is_container() || !method.elem.allows(&elem) {
                return None;
            }
            if method.recv == RecvClass::Set && !elem.is_key() {
                return None;
            }
            Some(method.recv.wrap(elem)?)
        }
        (None, None) => None,
    };
    if let Some(ty) = &recv
        && let Some(elem) = inner_elem(method.recv, ty)
        && !method.elem.allows(elem)
    {
        return None;
    }
    if found.fish.is_some() && method.fish == FishReq::None {
        return None;
    }
    if let Some(fish) = &found.fish
        && !fish_allows(method.fish, fish)
    {
        return None;
    }
    Some(Solved {
        recv,
        fish: found.fish,
        key: None,
        val: None,
    })
}

/// The inner element for `Vec<Vec<E>>`.
fn inner_elem(recv: RecvClass, ty: &Ty) -> Option<&Ty> {
    match recv {
        RecvClass::VecOfVec => ty.elem()?.elem(),
        _ => ty.elem(),
    }
}

/// A fully pinned pair becomes the receiver, a half pinned one guides the sample. A map key must
/// hash and a map value must sort.
fn solve_pair(method: &Method, found: Found) -> Option<Solved> {
    if found.same.is_some() || found.elem.is_some() || found.fish.is_some() {
        return None;
    }
    if method.fish != FishReq::None {
        return None;
    }
    if method.recv == RecvClass::Map {
        if found.key.as_ref().is_some_and(|key| !key.is_key()) {
            return None;
        }
        if found.val.as_ref().is_some_and(|val| !is_map_val(val)) {
            return None;
        }
    }
    let recv = match (&found.key, &found.val) {
        (Some(key), Some(val)) if method.recv == RecvClass::Map => {
            Some(Ty::map_of(key.clone(), val.clone()))
        }
        (Some(ok), Some(err)) => Some(Ty::res_of(ok.clone(), err.clone())),
        _ => None,
    };
    Some(Solved {
        recv,
        fish: None,
        key: found.key,
        val: found.val,
    })
}

pub fn is_map_val(ty: &Ty) -> bool {
    ty.is_ord() && ty.has_default()
}

pub fn fish_allows(req: FishReq, ty: &Ty) -> bool {
    match req {
        FishReq::None => false,
        FishReq::ParseTarget => matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char),
        FishReq::Scalar => !matches!(ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_)),
    }
}

#[derive(Default)]
struct Found {
    same: Option<Ty>,
    elem: Option<Ty>,
    fish: Option<Ty>,
    key: Option<Ty>,
    val: Option<Ty>,
}

fn unify(pat: &TyPat, want: &Ty, found: &mut Found) -> Option<()> {
    match pat {
        Same => {
            if found.same.as_ref().is_some_and(|seen| seen != want) {
                return None;
            }
            found.same = Some(want.clone());
            Some(())
        }
        Elem => {
            if found.elem.as_ref().is_some_and(|seen| seen != want) {
                return None;
            }
            found.elem = Some(want.clone());
            Some(())
        }
        TyPat::Key | OkT => {
            found.key = Some(want.clone());
            Some(())
        }
        TyPat::Val | ErrT => {
            found.val = Some(want.clone());
            Some(())
        }
        Exact(fixed) => (fixed.ty() == *want).then_some(()),
        Fish => {
            found.fish = Some(want.clone());
            Some(())
        }
        TyPat::Vec(inner) => match want {
            Ty::Vec(elem) => unify(inner, elem, found),
            _ => None,
        },
        TyPat::Opt(inner) => match want {
            Ty::Opt(elem) => unify(inner, elem, found),
            _ => None,
        },
        TyPat::Tuple2(first, second) => match want {
            Ty::Tuple(items) if items.len() == 2 => {
                unify(first, &items[0], found)?;
                unify(second, &items[1], found)
            }
            _ => None,
        },
        TyPat::Res(ok, err) => match want {
            Ty::Res(want_ok, want_err) => {
                unify(ok, want_ok, found)?;
                unify(err, want_err, found)
            }
            _ => None,
        },
        SmallU32 | SmallI32 | SmallUsize => None,
    }
}

/// `Elem` is the inner element only for a `VecOfVec` method, so `vec_contains` on a
/// `Vec<Vec<u64>>` still takes a `Vec<u64>`.
pub fn arg_ty(pat: &TyPat, class: RecvClass, recv: &Ty, fish: Option<&Ty>) -> Option<Ty> {
    Some(match pat {
        Same => recv.clone(),
        Elem => inner_elem(class, recv)?.clone(),
        TyPat::Key => recv.key_val()?.0.clone(),
        TyPat::Val => recv.key_val()?.1.clone(),
        OkT => recv.ok_err()?.0.clone(),
        ErrT => recv.ok_err()?.1.clone(),
        Exact(fixed) => fixed.ty(),
        Fish => fish?.clone(),
        TyPat::Vec(inner) => Ty::vec_of(arg_ty(inner, class, recv, fish)?),
        TyPat::Opt(inner) => Ty::opt_of(arg_ty(inner, class, recv, fish)?),
        TyPat::Tuple2(first, second) => Ty::Tuple(vec![
            arg_ty(first, class, recv, fish)?,
            arg_ty(second, class, recv, fish)?,
        ]),
        TyPat::Res(ok, err) => Ty::res_of(
            arg_ty(ok, class, recv, fish)?,
            arg_ty(err, class, recv, fish)?,
        ),
        SmallU32 => Ty::Int(IntWidth::U32),
        SmallI32 => Ty::Int(IntWidth::I32),
        SmallUsize => Ty::Int(IntWidth::USize),
    })
}
