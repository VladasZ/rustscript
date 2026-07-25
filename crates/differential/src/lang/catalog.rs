//! The typed method catalog.
//!
//! Every method is one row: a receiver class, argument type patterns, a result
//! type pattern, and a render template. The generator asks "what can produce a
//! `u8`" and the solver answers with every row whose result unifies, plus the
//! receiver type each one needs. So a new method is a single row and it
//! immediately composes with everything else, at any depth, inside any
//! expression.
//!
//! That is the difference from the old `method_case.rs`, where each method was
//! a hand written enum variant with its own render arm and its own shrink arm,
//! printed as a standalone labeled line that never met the rest of the program.

use crate::lang::ty::Ty;
use crate::numeric::{FloatWidth, IntWidth};

/// Which receiver types a method applies to.
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
    Opt,
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
            Self::Opt => matches!(ty, Ty::Opt(_)),
        }
    }

    /// Whether this class wraps an element type, so an `Elem` result can name
    /// the receiver as a container over the wanted type.
    pub fn is_container(self) -> bool {
        matches!(self, Self::Vec | Self::Opt)
    }

    pub fn wrap(self, elem: Ty) -> Option<Ty> {
        match self {
            Self::Vec => Some(Ty::vec_of(elem)),
            Self::Opt => Some(Ty::opt_of(elem)),
            _ => None,
        }
    }
}

/// A type in a method signature, written relative to the receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TyPat {
    /// The receiver's own type.
    Same,
    /// The element type of a `Vec<E>` or `Option<E>` receiver.
    Elem,
    /// A fixed scalar type that carries no type variable.
    Exact(Fixed),
    Vec(&'static TyPat),
    Opt(&'static TyPat),
    /// A turbofish type the generator picks, as in `parse::<u8>()`.
    Fish,
    /// A small literal count, so `repeat` and `pow` cannot blow up runtime.
    SmallU32,
    SmallI32,
    SmallUsize,
}

/// Scalar types nameable in a signature without a type variable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fixed {
    Bool,
    Char,
    Str,
    U32,
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
            Self::USize => Ty::Int(IntWidth::USize),
            Self::F64 => Ty::Float(FloatWidth::F64),
        }
    }
}

/// A constraint the element type of a container receiver must satisfy, so the
/// generated call actually compiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElemReq {
    Any,
    /// `Ord`, which every generated type has except the floats.
    Ord,
    /// An integer or a float, for `sum` and `product`.
    Num,
}

impl ElemReq {
    fn allows(self, ty: &Ty) -> bool {
        match self {
            Self::Any => true,
            Self::Ord => is_ord(ty),
            Self::Num => ty.is_numeric(),
        }
    }
}

fn is_ord(ty: &Ty) -> bool {
    match ty {
        Ty::Float(_) => false,
        Ty::Vec(inner) | Ty::Opt(inner) => is_ord(inner),
        _ => true,
    }
}

/// What a turbofish may be instantiated to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FishReq {
    None,
    /// Any type `FromStr` covers here: integers, floats, bool and char.
    ParseTarget,
    /// Any scalar, for `then_some`.
    Scalar,
}

pub struct Method {
    pub name: &'static str,
    pub recv: RecvClass,
    pub args: &'static [TyPat],
    pub ret: TyPat,
    pub elem: ElemReq,
    pub fish: FishReq,
    /// `{r}` receiver, `{0}`.. arguments, `{E}` element type, `{T}` turbofish.
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

use ElemReq::{Num, Ord as OrdElem};
use Fixed::{Bool as FBool, F64 as FF64, Str as FStr, U32 as FU32, USize as FUSize};
use RecvClass::{Char, Float, Int, Opt, SignedInt, Str, UnsignedInt, Vec as VecRecv};
use TyPat::{Elem, Exact, Fish, Same, SmallI32, SmallU32, SmallUsize};

const SAME: &TyPat = &Same;
const ELEM: &TyPat = &Elem;
const USIZE_PAT: &TyPat = &Exact(FUSize);

pub const METHODS: &[Method] = &[
    // -- integers, every width ---------------------------------------------
    m(
        "saturating_add",
        Int,
        &[Same],
        Same,
        "{r}.saturating_add({0})",
    ),
    m(
        "saturating_sub",
        Int,
        &[Same],
        Same,
        "{r}.saturating_sub({0})",
    ),
    m(
        "saturating_mul",
        Int,
        &[Same],
        Same,
        "{r}.saturating_mul({0})",
    ),
    m("wrapping_add", Int, &[Same], Same, "{r}.wrapping_add({0})"),
    m("wrapping_sub", Int, &[Same], Same, "{r}.wrapping_sub({0})"),
    m("wrapping_mul", Int, &[Same], Same, "{r}.wrapping_mul({0})"),
    m(
        "checked_add",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_add({0})",
    ),
    m(
        "checked_sub",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_sub({0})",
    ),
    m(
        "checked_mul",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_mul({0})",
    ),
    m(
        "checked_div",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_div({0})",
    ),
    m(
        "checked_rem",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_rem({0})",
    ),
    m("pow", Int, &[SmallU32], Same, "{r}.pow({0})"),
    m("min", Int, &[Same], Same, "{r}.min({0})"),
    m("max", Int, &[Same], Same, "{r}.max({0})"),
    m("div_euclid", Int, &[Same], Same, "{r}.div_euclid({0})"),
    m("rem_euclid", Int, &[Same], Same, "{r}.rem_euclid({0})"),
    m("count_ones", Int, &[], Exact(FU32), "{r}.count_ones()"),
    m("count_zeros", Int, &[], Exact(FU32), "{r}.count_zeros()"),
    m(
        "leading_zeros",
        Int,
        &[],
        Exact(FU32),
        "{r}.leading_zeros()",
    ),
    m(
        "trailing_zeros",
        Int,
        &[],
        Exact(FU32),
        "{r}.trailing_zeros()",
    ),
    m(
        "rotate_left",
        Int,
        &[SmallU32],
        Same,
        "{r}.rotate_left({0})",
    ),
    m(
        "rotate_right",
        Int,
        &[SmallU32],
        Same,
        "{r}.rotate_right({0})",
    ),
    m("swap_bytes", Int, &[], Same, "{r}.swap_bytes()"),
    m("reverse_bits", Int, &[], Same, "{r}.reverse_bits()"),
    m("isqrt", Int, &[], Same, "{r}.isqrt()"),
    m("int_to_string", Int, &[], Exact(FStr), "{r}.to_string()"),
    m("abs", SignedInt, &[], Same, "{r}.abs()"),
    m("signum", SignedInt, &[], Same, "{r}.signum()"),
    m(
        "checked_neg",
        SignedInt,
        &[],
        TyPat::Opt(SAME),
        "{r}.checked_neg()",
    ),
    m(
        "is_multiple_of",
        UnsignedInt,
        &[Same],
        Exact(FBool),
        "{r}.is_multiple_of({0})",
    ),
    // -- floats -------------------------------------------------------------
    m("float_abs", Float, &[], Same, "{r}.abs()"),
    m("sqrt", Float, &[], Same, "{r}.sqrt()"),
    m("floor", Float, &[], Same, "{r}.floor()"),
    m("ceil", Float, &[], Same, "{r}.ceil()"),
    m("round", Float, &[], Same, "{r}.round()"),
    m("trunc", Float, &[], Same, "{r}.trunc()"),
    m("fract", Float, &[], Same, "{r}.fract()"),
    m("float_signum", Float, &[], Same, "{r}.signum()"),
    m("recip", Float, &[], Same, "{r}.recip()"),
    m("powi", Float, &[SmallI32], Same, "{r}.powi({0})"),
    m("powf", Float, &[Same], Same, "{r}.powf({0})"),
    m("float_min", Float, &[Same], Same, "{r}.min({0})"),
    m("float_max", Float, &[Same], Same, "{r}.max({0})"),
    m(
        "mul_add",
        Float,
        &[Same, Same],
        Same,
        "{r}.mul_add({0}, {1})",
    ),
    m("is_nan", Float, &[], Exact(FBool), "{r}.is_nan()"),
    m("is_finite", Float, &[], Exact(FBool), "{r}.is_finite()"),
    m("is_infinite", Float, &[], Exact(FBool), "{r}.is_infinite()"),
    m(
        "is_sign_positive",
        Float,
        &[],
        Exact(FBool),
        "{r}.is_sign_positive()",
    ),
    m(
        "is_sign_negative",
        Float,
        &[],
        Exact(FBool),
        "{r}.is_sign_negative()",
    ),
    m(
        "float_to_string",
        Float,
        &[],
        Exact(FStr),
        "{r}.to_string()",
    ),
    // -- strings ------------------------------------------------------------
    m("len", Str, &[], Exact(FUSize), "{r}.len()"),
    m("is_empty", Str, &[], Exact(FBool), "{r}.is_empty()"),
    m("to_uppercase", Str, &[], Exact(FStr), "{r}.to_uppercase()"),
    m("to_lowercase", Str, &[], Exact(FStr), "{r}.to_lowercase()"),
    m("trim", Str, &[], Exact(FStr), "{r}.trim().to_string()"),
    m(
        "trim_start",
        Str,
        &[],
        Exact(FStr),
        "{r}.trim_start().to_string()",
    ),
    m(
        "trim_end",
        Str,
        &[],
        Exact(FStr),
        "{r}.trim_end().to_string()",
    ),
    m(
        "contains",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.contains({0}.as_str())",
    ),
    m(
        "starts_with",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.starts_with({0}.as_str())",
    ),
    m(
        "ends_with",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.ends_with({0}.as_str())",
    ),
    m(
        "find",
        Str,
        &[Exact(FStr)],
        TyPat::Opt(USIZE_PAT),
        "{r}.find({0}.as_str())",
    ),
    m(
        "replace",
        Str,
        &[Exact(FStr), Exact(FStr)],
        Exact(FStr),
        "{r}.replace({0}.as_str(), {1}.as_str())",
    ),
    m("repeat", Str, &[SmallUsize], Exact(FStr), "{r}.repeat({0})"),
    m(
        "chars_count",
        Str,
        &[],
        Exact(FUSize),
        "{r}.chars().count()",
    ),
    m(
        "split_count",
        Str,
        &[Exact(FStr)],
        Exact(FUSize),
        "{r}.split({0}.as_str()).count()",
    ),
    m(
        "eq_ignore_ascii_case",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.eq_ignore_ascii_case({0}.as_str())",
    ),
    // The parse family is why the turbofish exists. It is the one method whose
    // result type is chosen by the caller, and the interpreter has to honor it.
    with_fish(
        m(
            "parse",
            Str,
            &[],
            TyPat::Opt(&Fish),
            "{r}.parse::<{T}>().ok()",
        ),
        FishReq::ParseTarget,
    ),
    with_fish(
        m(
            "parse_is_err",
            Str,
            &[],
            Exact(FBool),
            "{r}.parse::<{T}>().is_err()",
        ),
        FishReq::ParseTarget,
    ),
    // -- bool ---------------------------------------------------------------
    with_fish(
        m(
            "then_some",
            RecvClass::Bool,
            &[Fish],
            TyPat::Opt(&Fish),
            "{r}.then_some({0})",
        ),
        FishReq::Scalar,
    ),
    m(
        "bool_to_string",
        RecvClass::Bool,
        &[],
        Exact(FStr),
        "{r}.to_string()",
    ),
    // -- char ---------------------------------------------------------------
    m(
        "is_alphabetic",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_alphabetic()",
    ),
    m("is_numeric", Char, &[], Exact(FBool), "{r}.is_numeric()"),
    m(
        "is_alphanumeric",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_alphanumeric()",
    ),
    m(
        "is_whitespace",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_whitespace()",
    ),
    m("is_ascii", Char, &[], Exact(FBool), "{r}.is_ascii()"),
    m(
        "is_uppercase",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_uppercase()",
    ),
    m(
        "is_lowercase",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_lowercase()",
    ),
    m(
        "to_ascii_uppercase",
        Char,
        &[],
        Same,
        "{r}.to_ascii_uppercase()",
    ),
    m(
        "to_ascii_lowercase",
        Char,
        &[],
        Same,
        "{r}.to_ascii_lowercase()",
    ),
    m("char_to_string", Char, &[], Exact(FStr), "{r}.to_string()"),
    m(
        "char_to_digit",
        Char,
        &[],
        TyPat::Opt(&Exact(FU32)),
        "{r}.to_digit(10)",
    ),
    // -- Vec ----------------------------------------------------------------
    m("vec_len", VecRecv, &[], Exact(FUSize), "{r}.len()"),
    m("vec_is_empty", VecRecv, &[], Exact(FBool), "{r}.is_empty()"),
    m(
        "first",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.first().cloned()",
    ),
    m(
        "last",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.last().cloned()",
    ),
    m(
        "get",
        VecRecv,
        &[SmallUsize],
        TyPat::Opt(ELEM),
        "{r}.get({0}).cloned()",
    ),
    m(
        "vec_contains",
        VecRecv,
        &[Elem],
        Exact(FBool),
        "{r}.contains(&{0})",
    ),
    with_elem(
        m(
            "vec_max",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.iter().max().cloned()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_min",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.iter().min().cloned()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_sum",
            VecRecv,
            &[],
            Elem,
            "{r}.iter().copied().sum::<{E}>()",
        ),
        Num,
    ),
    with_elem(
        m(
            "vec_sorted",
            VecRecv,
            &[],
            Same,
            "({{ let mut sorted = {r}; sorted.sort(); sorted }})",
        ),
        OrdElem,
    ),
    m(
        "vec_reversed",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().rev().collect::<Vec<{E}>>()",
    ),
    // -- Option -------------------------------------------------------------
    m("is_some", Opt, &[], Exact(FBool), "{r}.is_some()"),
    m("is_none", Opt, &[], Exact(FBool), "{r}.is_none()"),
    m("unwrap_or", Opt, &[Elem], Elem, "{r}.unwrap_or({0})"),
    m(
        "unwrap_or_default",
        Opt,
        &[],
        Elem,
        "{r}.unwrap_or_default()",
    ),
    m("opt_or", Opt, &[Same], Same, "{r}.or({0})"),
    m(
        "opt_to_vec",
        Opt,
        &[],
        TyPat::Vec(ELEM),
        "{r}.into_iter().collect::<Vec<{E}>>()",
    ),
    m(
        "opt_as_f64",
        Opt,
        &[],
        Exact(FF64),
        "(({r}.is_some() as u8) as f64)",
    ),
];

/// What solving a result pattern against a wanted type told us about the call.
#[derive(Clone, Debug)]
pub struct Solved {
    /// The receiver type, when the wanted type pinned it. `None` means any
    /// type in the method's receiver class works and the generator picks.
    pub recv: Option<Ty>,
    pub fish: Option<Ty>,
}

/// Solve a method's result pattern against the type the generator wants.
/// Returns the receiver the call must have, or `None` when this method can
/// never produce that type.
pub fn solve(method: &Method, want: &Ty) -> Option<Solved> {
    let mut found = Found::default();
    unify(&method.ret, want, &mut found)?;
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
            Some(method.recv.wrap(elem)?)
        }
        (None, None) => None,
    };
    if let Some(ty) = &recv
        && let Some(elem) = ty.elem()
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
    })
}

pub fn fish_allows(req: FishReq, ty: &Ty) -> bool {
    match req {
        FishReq::None => false,
        FishReq::ParseTarget => matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char),
        FishReq::Scalar => !matches!(ty, Ty::Vec(_)),
    }
}

#[derive(Default)]
struct Found {
    same: Option<Ty>,
    elem: Option<Ty>,
    fish: Option<Ty>,
}

fn unify(pat: &TyPat, want: &Ty, found: &mut Found) -> Option<()> {
    match pat {
        Same => {
            found.same = Some(want.clone());
            Some(())
        }
        Elem => {
            found.elem = Some(want.clone());
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
        // Count patterns describe an argument, never a result.
        SmallU32 | SmallI32 | SmallUsize => None,
    }
}

/// The concrete type an argument pattern takes for a solved call.
pub fn arg_ty(pat: &TyPat, recv: &Ty, fish: Option<&Ty>) -> Option<Ty> {
    Some(match pat {
        Same => recv.clone(),
        Elem => recv.elem()?.clone(),
        Exact(fixed) => fixed.ty(),
        Fish => fish?.clone(),
        TyPat::Vec(inner) => Ty::vec_of(arg_ty(inner, recv, fish)?),
        TyPat::Opt(inner) => Ty::opt_of(arg_ty(inner, recv, fish)?),
        SmallU32 => Ty::Int(IntWidth::U32),
        SmallI32 => Ty::Int(IntWidth::I32),
        SmallUsize => Ty::Int(IntWidth::USize),
    })
}
