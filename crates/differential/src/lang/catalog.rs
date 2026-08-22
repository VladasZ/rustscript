//! The typed method catalog. Every method is one row, a receiver class,
//! argument patterns, a result pattern and a template. A new row composes
//! at any depth at once. The `surface` command measures the gap against
//! `std_surface.txt`.

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
    /// `Vec<Vec<E>>`, for `concat` and friends.
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

    /// Whether an `Elem` result can name the receiver as a container over
    /// the wanted type. `Map` and `Res` need 2 types and are completed in
    /// `solve`.
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
    /// The element type, or the inner element of a `Vec<Vec<E>>`.
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
    /// A turbofish type the generator picks.
    Fish,
    /// A small literal, so `repeat` and `pow` cannot blow up runtime.
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
    /// `Ord`, every generated type except the floats.
    Ord,
    /// An integer or a float, for `sum` and `product`.
    Num,
    /// Hashable, `Eq`, and ordered, for a set or a map key.
    Key,
    /// `Default`, for `unwrap_or_default`.
    Default,
    /// Exactly `String`, for `join` and `concat`.
    Str,
    /// `Copy`, for slice `repeat`.
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
    /// Any `FromStr` type.
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
    /// `{r}` receiver, `{0}`.. arguments, `{E}` element type, `{T}` turbofish,
    /// `{K}` and `{V}` the key and value or the ok and error types.
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
    m(
        "saturating_pow",
        Int,
        &[SmallU32],
        Same,
        "{r}.saturating_pow({0})",
    ),
    m("wrapping_add", Int, &[Same], Same, "{r}.wrapping_add({0})"),
    m("wrapping_sub", Int, &[Same], Same, "{r}.wrapping_sub({0})"),
    m("wrapping_mul", Int, &[Same], Same, "{r}.wrapping_mul({0})"),
    m(
        "wrapping_pow",
        Int,
        &[SmallU32],
        Same,
        "{r}.wrapping_pow({0})",
    ),
    m(
        "wrapping_shl",
        Int,
        &[SmallU32],
        Same,
        "{r}.wrapping_shl({0})",
    ),
    m(
        "wrapping_shr",
        Int,
        &[SmallU32],
        Same,
        "{r}.wrapping_shr({0})",
    ),
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
    m(
        "checked_pow",
        Int,
        &[SmallU32],
        TyPat::Opt(SAME),
        "{r}.checked_pow({0})",
    ),
    m(
        "checked_shl",
        Int,
        &[SmallU32],
        TyPat::Opt(SAME),
        "{r}.checked_shl({0})",
    ),
    m(
        "checked_shr",
        Int,
        &[SmallU32],
        TyPat::Opt(SAME),
        "{r}.checked_shr({0})",
    ),
    m(
        "checked_rem_euclid",
        Int,
        &[Same],
        TyPat::Opt(SAME),
        "{r}.checked_rem_euclid({0})",
    ),
    m(
        "checked_ilog2",
        Int,
        &[],
        TyPat::Opt(U32_PAT),
        "{r}.checked_ilog2()",
    ),
    m(
        "overflowing_add",
        Int,
        &[Same],
        TyPat::Tuple2(SAME, BOOL_PAT),
        "{r}.overflowing_add({0})",
    ),
    m(
        "overflowing_sub",
        Int,
        &[Same],
        TyPat::Tuple2(SAME, BOOL_PAT),
        "{r}.overflowing_sub({0})",
    ),
    m(
        "overflowing_mul",
        Int,
        &[Same],
        TyPat::Tuple2(SAME, BOOL_PAT),
        "{r}.overflowing_mul({0})",
    ),
    m("pow", Int, &[SmallU32], Same, "{r}.pow({0})"),
    m("min", Int, &[Same], Same, "{r}.min({0})"),
    m("max", Int, &[Same], Same, "{r}.max({0})"),
    // Panics when min > max, exactly like debug Rust.
    m("clamp", Int, &[Same, Same], Same, "{r}.clamp({0}, {1})"),
    m("midpoint", Int, &[Same], Same, "{r}.midpoint({0})"),
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
    m("leading_ones", Int, &[], Exact(FU32), "{r}.leading_ones()"),
    m(
        "trailing_ones",
        Int,
        &[],
        Exact(FU32),
        "{r}.trailing_ones()",
    ),
    // Panics on zero and on a negative value.
    m("ilog2", Int, &[], Exact(FU32), "{r}.ilog2()"),
    m("ilog10", Int, &[], Exact(FU32), "{r}.ilog10()"),
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
        "is_negative",
        SignedInt,
        &[],
        Exact(FBool),
        "{r}.is_negative()",
    ),
    m(
        "is_positive",
        SignedInt,
        &[],
        Exact(FBool),
        "{r}.is_positive()",
    ),
    m(
        "checked_neg",
        SignedInt,
        &[],
        TyPat::Opt(SAME),
        "{r}.checked_neg()",
    ),
    m(
        "checked_abs",
        SignedInt,
        &[],
        TyPat::Opt(SAME),
        "{r}.checked_abs()",
    ),
    m("wrapping_neg", SignedInt, &[], Same, "{r}.wrapping_neg()"),
    m("wrapping_abs", SignedInt, &[], Same, "{r}.wrapping_abs()"),
    m(
        "is_multiple_of",
        UnsignedInt,
        &[Same],
        Exact(FBool),
        "{r}.is_multiple_of({0})",
    ),
    m(
        "is_power_of_two",
        UnsignedInt,
        &[],
        Exact(FBool),
        "{r}.is_power_of_two()",
    ),
    // Panics past the top power in debug Rust.
    m(
        "next_power_of_two",
        UnsignedInt,
        &[],
        Same,
        "{r}.next_power_of_two()",
    ),
    m("div_ceil", UnsignedInt, &[Same], Same, "{r}.div_ceil({0})"),
    m(
        "next_multiple_of",
        UnsignedInt,
        &[Same],
        Same,
        "{r}.next_multiple_of({0})",
    ),
    // -- floats -------------------------------------------------------------
    m("float_abs", Float, &[], Same, "{r}.abs()"),
    m("sqrt", Float, &[], Same, "{r}.sqrt()"),
    m("cbrt", Float, &[], Same, "{r}.cbrt()"),
    m("floor", Float, &[], Same, "{r}.floor()"),
    m("ceil", Float, &[], Same, "{r}.ceil()"),
    m("round", Float, &[], Same, "{r}.round()"),
    m("round_ties_even", Float, &[], Same, "{r}.round_ties_even()"),
    m("trunc", Float, &[], Same, "{r}.trunc()"),
    m("fract", Float, &[], Same, "{r}.fract()"),
    m("float_signum", Float, &[], Same, "{r}.signum()"),
    m("recip", Float, &[], Same, "{r}.recip()"),
    m("exp", Float, &[], Same, "{r}.exp()"),
    m("exp2", Float, &[], Same, "{r}.exp2()"),
    m("ln", Float, &[], Same, "{r}.ln()"),
    m("log2", Float, &[], Same, "{r}.log2()"),
    m("log10", Float, &[], Same, "{r}.log10()"),
    m("to_degrees", Float, &[], Same, "{r}.to_degrees()"),
    m("to_radians", Float, &[], Same, "{r}.to_radians()"),
    m("powi", Float, &[SmallI32], Same, "{r}.powi({0})"),
    m("powf", Float, &[Same], Same, "{r}.powf({0})"),
    m("hypot", Float, &[Same], Same, "{r}.hypot({0})"),
    m("copysign", Float, &[Same], Same, "{r}.copysign({0})"),
    m("float_min", Float, &[Same], Same, "{r}.min({0})"),
    m("float_max", Float, &[Same], Same, "{r}.max({0})"),
    // Panics when min > max or either is NaN.
    m(
        "float_clamp",
        Float,
        &[Same, Same],
        Same,
        "{r}.clamp({0}, {1})",
    ),
    m("float_midpoint", Float, &[Same], Same, "{r}.midpoint({0})"),
    m(
        "float_rem_euclid",
        Float,
        &[Same],
        Same,
        "{r}.rem_euclid({0})",
    ),
    m(
        "float_div_euclid",
        Float,
        &[Same],
        Same,
        "{r}.div_euclid({0})",
    ),
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
    m("is_normal", Float, &[], Exact(FBool), "{r}.is_normal()"),
    m(
        "is_subnormal",
        Float,
        &[],
        Exact(FBool),
        "{r}.is_subnormal()",
    ),
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
    m(
        "str_to_ascii_uppercase",
        Str,
        &[],
        Exact(FStr),
        "{r}.to_ascii_uppercase()",
    ),
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
        "trim_matches",
        Str,
        &[Exact(FChar)],
        Exact(FStr),
        "{r}.trim_matches({0}).to_string()",
    ),
    m(
        "contains",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.contains({0}.as_str())",
    ),
    m(
        "contains_char",
        Str,
        &[Exact(FChar)],
        Exact(FBool),
        "{r}.contains({0})",
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
        "rfind",
        Str,
        &[Exact(FStr)],
        TyPat::Opt(USIZE_PAT),
        "{r}.rfind({0}.as_str())",
    ),
    m(
        "replace",
        Str,
        &[Exact(FStr), Exact(FStr)],
        Exact(FStr),
        "{r}.replace({0}.as_str(), {1}.as_str())",
    ),
    m(
        "replacen",
        Str,
        &[Exact(FStr), Exact(FStr), SmallUsize],
        Exact(FStr),
        "{r}.replacen({0}.as_str(), {1}.as_str(), {2})",
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
        "chars_rev",
        Str,
        &[],
        Exact(FStr),
        "{r}.chars().rev().collect::<String>()",
    ),
    m(
        "chars_nth",
        Str,
        &[SmallUsize],
        TyPat::Opt(CHAR_PAT),
        "{r}.chars().nth({0})",
    ),
    m(
        "chars_collect",
        Str,
        &[],
        TyPat::Vec(CHAR_PAT),
        "{r}.chars().collect::<Vec<char>>()",
    ),
    m(
        "bytes_sum",
        Str,
        &[],
        Exact(FU64),
        "{r}.bytes().map(u64::from).sum::<u64>()",
    ),
    m(
        "split_count",
        Str,
        &[Exact(FStr)],
        Exact(FUSize),
        "{r}.split({0}.as_str()).count()",
    ),
    m(
        "split_collect",
        Str,
        &[Exact(FStr)],
        TyPat::Vec(STR_PAT),
        "{r}.split({0}.as_str()).map(String::from).collect::<Vec<String>>()",
    ),
    m(
        "split_whitespace_collect",
        Str,
        &[],
        TyPat::Vec(STR_PAT),
        "{r}.split_whitespace().map(String::from).collect::<Vec<String>>()",
    ),
    m(
        "lines_collect",
        Str,
        &[],
        TyPat::Vec(STR_PAT),
        "{r}.lines().map(String::from).collect::<Vec<String>>()",
    ),
    m(
        "split_once",
        Str,
        &[Exact(FStr)],
        TyPat::Opt(&TyPat::Tuple2(STR_PAT, STR_PAT)),
        "{r}.split_once({0}.as_str()).map(|(a, b)| (a.to_string(), b.to_string()))",
    ),
    m(
        "strip_prefix",
        Str,
        &[Exact(FStr)],
        TyPat::Opt(STR_PAT),
        "{r}.strip_prefix({0}.as_str()).map(String::from)",
    ),
    m(
        "strip_suffix",
        Str,
        &[Exact(FStr)],
        TyPat::Opt(STR_PAT),
        "{r}.strip_suffix({0}.as_str()).map(String::from)",
    ),
    m(
        "str_get",
        Str,
        &[SmallUsize],
        TyPat::Opt(STR_PAT),
        "{r}.get(0..{0}).map(String::from)",
    ),
    m(
        "is_char_boundary",
        Str,
        &[SmallUsize],
        Exact(FBool),
        "{r}.is_char_boundary({0})",
    ),
    m(
        "matches_count",
        Str,
        &[Exact(FStr)],
        Exact(FUSize),
        "{r}.matches({0}.as_str()).count()",
    ),
    m(
        "eq_ignore_ascii_case",
        Str,
        &[Exact(FStr)],
        Exact(FBool),
        "{r}.eq_ignore_ascii_case({0}.as_str())",
    ),
    m("is_ascii_str", Str, &[], Exact(FBool), "{r}.is_ascii()"),
    // The parse family is why the turbofish exists, the caller chooses the
    // result type.
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
    with_fish(
        m(
            "parse_result",
            Str,
            &[],
            TyPat::Res(&Fish, STR_PAT),
            "{r}.parse::<{T}>().map_err(|e| e.to_string())",
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
        "is_ascii_digit",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_ascii_digit()",
    ),
    m(
        "is_ascii_alphabetic",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_ascii_alphabetic()",
    ),
    m(
        "is_ascii_punctuation",
        Char,
        &[],
        Exact(FBool),
        "{r}.is_ascii_punctuation()",
    ),
    m("is_control", Char, &[], Exact(FBool), "{r}.is_control()"),
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
    m("len_utf8", Char, &[], Exact(FUSize), "{r}.len_utf8()"),
    m(
        "char_to_digit",
        Char,
        &[],
        TyPat::Opt(U32_PAT),
        "{r}.to_digit(10)",
    ),
    m(
        "char_to_digit_16",
        Char,
        &[],
        TyPat::Opt(U32_PAT),
        "{r}.to_digit(16)",
    ),
    m(
        "char_eq_ignore_ascii_case",
        Char,
        &[Same],
        Exact(FBool),
        "{r}.eq_ignore_ascii_case(&{0})",
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
    // Panics out of bounds.
    m(
        "vec_index",
        VecRecv,
        &[SmallUsize],
        Elem,
        "{r}[{0}].clone()",
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
            "vec_product",
            VecRecv,
            &[],
            Elem,
            "{r}.iter().copied().product::<{E}>()",
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
    with_elem(
        m(
            "vec_sorted_dedup",
            VecRecv,
            &[],
            Same,
            "({{ let mut sorted = {r}; sorted.sort(); sorted.dedup(); sorted }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_binary_search",
            VecRecv,
            &[Elem],
            TyPat::Res(USIZE_PAT, USIZE_PAT),
            "({{ let mut sorted = {r}; sorted.sort(); sorted.binary_search(&{0}) }})",
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
    m(
        "vec_take",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().take({0}).collect::<Vec<{E}>>()",
    ),
    m(
        "vec_skip",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().skip({0}).collect::<Vec<{E}>>()",
    ),
    // Panics on a zero step.
    m(
        "vec_step_by",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().step_by({0}).collect::<Vec<{E}>>()",
    ),
    with_elem(
        m(
            "vec_repeat",
            VecRecv,
            &[SmallUsize],
            Same,
            "{r}.repeat({0})",
        ),
        ElemReq::Copy,
    ),
    m(
        "vec_concat_with",
        VecRecv,
        &[Same],
        Same,
        "[{r}, {0}].concat()",
    ),
    // Panics when the split point passes the end.
    m(
        "vec_split_at",
        VecRecv,
        &[SmallUsize],
        TyPat::Tuple2(SAME, SAME),
        "({{ let (a, b) = {r}.split_at({0}); (a.to_vec(), b.to_vec()) }})",
    ),
    m(
        "vec_split_first",
        VecRecv,
        &[],
        TyPat::Opt(&TyPat::Tuple2(ELEM, SAME)),
        "{r}.split_first().map(|(a, b)| (a.clone(), b.to_vec()))",
    ),
    m(
        "vec_split_last",
        VecRecv,
        &[],
        TyPat::Opt(&TyPat::Tuple2(ELEM, SAME)),
        "{r}.split_last().map(|(a, b)| (a.clone(), b.to_vec()))",
    ),
    // Panics on a zero chunk size.
    m(
        "vec_chunk_lens",
        VecRecv,
        &[SmallUsize],
        TyPat::Vec(USIZE_PAT),
        "{r}.chunks({0}).map(<[{E}]>::len).collect::<Vec<usize>>()",
    ),
    m(
        "vec_window_count",
        VecRecv,
        &[SmallUsize],
        Exact(FUSize),
        "{r}.windows({0}).count()",
    ),
    with_elem(
        m(
            "vec_window_sums",
            VecRecv,
            &[],
            Same,
            "{r}.windows(2).map(|w| w[0] + w[1]).collect::<Vec<{E}>>()",
        ),
        Num,
    ),
    m(
        "vec_zip",
        VecRecv,
        &[Same],
        TyPat::Vec(&TyPat::Tuple2(ELEM, ELEM)),
        "{r}.into_iter().zip({0}).collect::<Vec<({E}, {E})>>()",
    ),
    m(
        "vec_enumerate",
        VecRecv,
        &[],
        TyPat::Vec(&TyPat::Tuple2(USIZE_PAT, ELEM)),
        "{r}.into_iter().enumerate().collect::<Vec<(usize, {E})>>()",
    ),
    with_elem(
        m(
            "vec_to_set_max",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.into_iter().collect::<HashSet<{E}>>().into_iter().max()",
        ),
        KeyElem,
    ),
    with_elem(
        m(
            "vec_join",
            VecRecv,
            &[Exact(FStr)],
            Exact(FStr),
            "{r}.join({0}.as_str())",
        ),
        StrElem,
    ),
    with_elem(
        m("vec_str_concat", VecRecv, &[], Exact(FStr), "{r}.concat()"),
        StrElem,
    ),
    m(
        "vec_concat",
        VecOfVec,
        &[],
        TyPat::Vec(ELEM),
        "{r}.concat()",
    ),
    m(
        "vec_flatten_len",
        VecOfVec,
        &[],
        Exact(FUSize),
        "{r}.into_iter().flatten().count()",
    ),
    // -- Option -------------------------------------------------------------
    m("is_some", Opt, &[], Exact(FBool), "{r}.is_some()"),
    m("is_none", Opt, &[], Exact(FBool), "{r}.is_none()"),
    m("unwrap_or", Opt, &[Elem], Elem, "{r}.unwrap_or({0})"),
    with_elem(
        m(
            "unwrap_or_default",
            Opt,
            &[],
            Elem,
            "{r}.unwrap_or_default()",
        ),
        DefaultElem,
    ),
    m("opt_or", Opt, &[Same], Same, "{r}.or({0})"),
    m("opt_xor", Opt, &[Same], Same, "{r}.xor({0})"),
    m("opt_and", Opt, &[Same], Same, "{r}.and({0})"),
    m(
        "opt_zip",
        Opt,
        &[Same],
        TyPat::Opt(&TyPat::Tuple2(ELEM, ELEM)),
        "{r}.zip({0})",
    ),
    m(
        "opt_ok_or",
        Opt,
        &[Exact(FStr)],
        TyPat::Res(ELEM, STR_PAT),
        "{r}.ok_or({0})",
    ),
    m(
        "opt_to_vec",
        Opt,
        &[],
        TyPat::Vec(ELEM),
        "{r}.into_iter().collect::<Vec<{E}>>()",
    ),
    m(
        "opt_iter_count",
        Opt,
        &[],
        Exact(FUSize),
        "{r}.iter().count()",
    ),
    m(
        "opt_as_f64",
        Opt,
        &[],
        Exact(FF64),
        "(({r}.is_some() as u8) as f64)",
    ),
    // -- Result -------------------------------------------------------------
    m("res_is_ok", Res, &[], Exact(FBool), "{r}.is_ok()"),
    m("res_is_err", Res, &[], Exact(FBool), "{r}.is_err()"),
    m("res_ok", Res, &[], TyPat::Opt(OK_PAT), "{r}.ok()"),
    m("res_err", Res, &[], TyPat::Opt(ERR_PAT), "{r}.err()"),
    m("res_unwrap_or", Res, &[OkT], OkT, "{r}.unwrap_or({0})"),
    m("res_and", Res, &[Same], Same, "{r}.and({0})"),
    m("res_or", Res, &[Same], Same, "{r}.or({0})"),
    // -- HashMap ------------------------------------------------------------
    // Anything that iterates sorts inside the template, see the determinism
    // rule in `pipe`.
    m("map_len", RecvClass::Map, &[], Exact(FUSize), "{r}.len()"),
    m(
        "map_is_empty",
        RecvClass::Map,
        &[],
        Exact(FBool),
        "{r}.is_empty()",
    ),
    m(
        "map_contains_key",
        RecvClass::Map,
        &[TyPat::Key],
        Exact(FBool),
        "{r}.contains_key(&{0})",
    ),
    m(
        "map_get_or",
        RecvClass::Map,
        &[TyPat::Key, TyPat::Val],
        TyPat::Val,
        "{r}.get(&{0}).cloned().unwrap_or({1})",
    ),
    m(
        "map_get_or_default",
        RecvClass::Map,
        &[TyPat::Key],
        TyPat::Val,
        "{r}.get(&{0}).cloned().unwrap_or_default()",
    ),
    m(
        "map_get",
        RecvClass::Map,
        &[TyPat::Key],
        TyPat::Opt(VAL_PAT),
        "{r}.get(&{0}).cloned()",
    ),
    m(
        "map_remove",
        RecvClass::Map,
        &[TyPat::Key],
        TyPat::Opt(VAL_PAT),
        "({{ let mut diff_owned = {r}; diff_owned.remove(&{0}) }})",
    ),
    m(
        "map_keys_max",
        RecvClass::Map,
        &[],
        TyPat::Opt(KEY_PAT),
        "{r}.into_keys().max()",
    ),
    m(
        "map_values_max",
        RecvClass::Map,
        &[],
        TyPat::Opt(VAL_PAT),
        "{r}.into_values().max()",
    ),
    m(
        "map_sorted_keys",
        RecvClass::Map,
        &[],
        TyPat::Vec(KEY_PAT),
        "({{ let mut diff_keys: Vec<{K}> = {r}.into_keys().collect(); diff_keys.sort(); diff_keys }})",
    ),
    m(
        "map_sorted_values",
        RecvClass::Map,
        &[],
        TyPat::Vec(VAL_PAT),
        "({{ let mut diff_values: Vec<{V}> = {r}.into_values().collect(); diff_values.sort(); diff_values }})",
    ),
    m(
        "map_sorted_pairs",
        RecvClass::Map,
        &[],
        TyPat::Vec(&TyPat::Tuple2(KEY_PAT, VAL_PAT)),
        "({{ let mut diff_pairs: Vec<({K}, {V})> = {r}.into_iter().collect(); diff_pairs.sort(); diff_pairs }})",
    ),
    // -- HashSet ------------------------------------------------------------
    m("set_len", RecvClass::Set, &[], Exact(FUSize), "{r}.len()"),
    m(
        "set_is_empty",
        RecvClass::Set,
        &[],
        Exact(FBool),
        "{r}.is_empty()",
    ),
    m(
        "set_contains",
        RecvClass::Set,
        &[Elem],
        Exact(FBool),
        "{r}.contains(&{0})",
    ),
    m(
        "set_is_subset",
        RecvClass::Set,
        &[Same],
        Exact(FBool),
        "{r}.is_subset(&{0})",
    ),
    m(
        "set_is_disjoint",
        RecvClass::Set,
        &[Same],
        Exact(FBool),
        "{r}.is_disjoint(&{0})",
    ),
    m(
        "set_insert_observed",
        RecvClass::Set,
        &[Elem],
        Exact(FBool),
        "({{ let mut diff_owned = {r}; diff_owned.insert({0}) }})",
    ),
    m(
        "set_max",
        RecvClass::Set,
        &[],
        TyPat::Opt(ELEM),
        "{r}.into_iter().max()",
    ),
    m(
        "set_sorted",
        RecvClass::Set,
        &[],
        TyPat::Vec(ELEM),
        "({{ let mut diff_elems: Vec<{E}> = {r}.into_iter().collect(); diff_elems.sort(); diff_elems }})",
    ),
    m(
        "set_union_sorted",
        RecvClass::Set,
        &[Same],
        TyPat::Vec(ELEM),
        "({{ let mut diff_elems: Vec<{E}> = {r}.union(&{0}).cloned().collect(); diff_elems.sort(); diff_elems }})",
    ),
    m(
        "set_intersection_sorted",
        RecvClass::Set,
        &[Same],
        TyPat::Vec(ELEM),
        "({{ let mut diff_elems: Vec<{E}> = {r}.intersection(&{0}).cloned().collect(); diff_elems.sort(); diff_elems }})",
    ),
    m(
        "set_difference_sorted",
        RecvClass::Set,
        &[Same],
        TyPat::Vec(ELEM),
        "({{ let mut diff_elems: Vec<{E}> = {r}.difference(&{0}).cloned().collect(); diff_elems.sort(); diff_elems }})",
    ),
];

#[derive(Clone, Debug)]
pub struct Solved {
    /// `None` means any type in the receiver class works and the generator
    /// picks, guided by `key` or `val` when half a pair is pinned.
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

/// A fully pinned pair becomes the receiver, a half pinned one guides the
/// sample. A map key must hash and a map value must sort.
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

/// `Elem` is the inner element only for a `VecOfVec` method, so
/// `vec_contains` on a `Vec<Vec<u64>>` still takes a `Vec<u64>`.
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
