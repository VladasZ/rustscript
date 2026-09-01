//! The numeric owner paths, `i64::from(x)`, `f64::max(a, b)`, `i32::MAX`, `f64::consts::PI`.

use syn::Expr;

use super::{Infer, Ty};
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    /// `i64::from(x)`, `u8::try_from(x)`, `f64::max(a, b)` and the other numeric paths.
    pub(super) fn numeric_call(&mut self, owner: &str, last: &str, args: &[&Expr]) -> Option<Ty> {
        Some(match (owner, last) {
            ("char", "from") => {
                self.walk_args(args);
                Ty::Char
            }
            ("char", "from_u32" | "from_digit") => {
                self.walk_args(args);
                Ty::option(Ty::Char)
            }
            ("f64", "from") => {
                self.walk_args(args);
                Ty::F64
            }
            ("f32", "from") => {
                self.walk_args(args);
                Ty::F32
            }
            (w, "from") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::Int(IntWidth::parse(w).expect("checked"))
            }
            (w, "try_from") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::result(
                    Ty::Int(IntWidth::parse(w).expect("checked")),
                    Ty::named("TryFromIntError"),
                )
            }
            (w, "from_str_radix") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::result(
                    Ty::Int(IntWidth::parse(w).expect("checked")),
                    Ty::named("ParseIntError"),
                )
            }
            (
                w,
                "max" | "min" | "pow" | "abs" | "saturating_sub" | "saturating_add"
                | "wrapping_add" | "wrapping_sub",
            ) if IntWidth::parse(w).is_some() => {
                let ty = Ty::Int(IntWidth::parse(w).expect("checked"));
                for arg in args {
                    self.expr(arg, &ty);
                }
                ty
            }
            (
                "f64" | "f32",
                "max" | "min" | "sqrt" | "abs" | "powi" | "powf" | "floor" | "ceil" | "round",
            ) => {
                let ty = if owner == "f32" { Ty::F32 } else { Ty::F64 };
                for arg in args {
                    self.expr(arg, &ty);
                }
                ty
            }
            _ => return None,
        })
    }
}

/// `i32::MAX`, `u8::MIN`, `f64::EPSILON`, `f64::consts::PI` and the like.
pub(super) fn numeric_constant(segs: &[String]) -> Option<Ty> {
    let (owner, name) = match segs {
        [.., owner, name] => (owner.as_str(), name.as_str()),
        _ => return None,
    };
    let owner = if owner == "consts" {
        match segs.len().checked_sub(3).and_then(|i| segs.get(i)) {
            Some(o) => o.as_str(),
            None => "f64",
        }
    } else {
        owner
    };
    match owner {
        "f64" => matches!(
            name,
            "MAX"
                | "MIN"
                | "EPSILON"
                | "INFINITY"
                | "NEG_INFINITY"
                | "NAN"
                | "PI"
                | "E"
                | "TAU"
                | "SQRT_2"
                | "LN_2"
                | "LN_10"
                | "FRAC_PI_2"
                | "MIN_POSITIVE"
        )
        .then_some(Ty::F64),
        "f32" => matches!(
            name,
            "MAX" | "MIN" | "EPSILON" | "INFINITY" | "NEG_INFINITY" | "NAN" | "PI" | "E" | "TAU"
        )
        .then_some(Ty::F32),
        "char" => matches!(name, "MAX" | "REPLACEMENT_CHARACTER").then_some(Ty::Char),
        other => {
            let width = IntWidth::parse(other)?;
            matches!(name, "MAX" | "MIN" | "BITS").then(|| {
                if name == "BITS" {
                    Ty::Int(IntWidth::U32)
                } else {
                    Ty::Int(width)
                }
            })
        }
    }
}
