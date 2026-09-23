//! A primitive field of a typed parse, read with the serde visitor of the primitive itself. A
//! value that does not fit, `300` for a `u8` or a string for a number, is the error serde gives,
//! and a number keeps the width of its field.

use serde::Deserialize;
use serde::de::{DeserializeSeed, Error, Visitor};

use super::numeric::IntWidth;
use super::typeir::ScalarIr;
use super::value::Value;

pub(super) fn scalar_seed<'de, D: serde::Deserializer<'de>>(
    d: D,
    scalar: ScalarIr,
    optional: bool,
) -> Result<Value, D::Error> {
    if optional {
        d.deserialize_option(OptionalScalar(scalar))
    } else {
        Scalar(scalar).deserialize(d)
    }
}

struct Scalar(ScalarIr);

impl<'de> DeserializeSeed<'de> for Scalar {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Ok(match self.0 {
            ScalarIr::Int(width) => Value::int_of_width(int_of(d, width)?, width),
            ScalarIr::F32 => Value::F32(f32::deserialize(d)?),
            ScalarIr::F64 => Value::Float(f64::deserialize(d)?),
            ScalarIr::Bool => Value::Bool(bool::deserialize(d)?),
            ScalarIr::Char => Value::Char(char::deserialize(d)?),
            ScalarIr::Str => Value::str(String::deserialize(d)?),
        })
    }
}

/// The value in the storage form of `width`, a u128 as its bits.
fn int_of<'de, D: serde::Deserializer<'de>>(d: D, width: IntWidth) -> Result<i128, D::Error> {
    Ok(match width {
        IntWidth::U8 => i128::from(u8::deserialize(d)?),
        IntWidth::U16 => i128::from(u16::deserialize(d)?),
        IntWidth::U32 => i128::from(u32::deserialize(d)?),
        IntWidth::U64 => i128::from(u64::deserialize(d)?),
        IntWidth::USize => {
            i128::try_from(usize::deserialize(d)?).map_err(|e| D::Error::custom(e.to_string()))?
        }
        IntWidth::U128 => u128::deserialize(d)?.cast_signed(),
        IntWidth::I8 => i128::from(i8::deserialize(d)?),
        IntWidth::I16 => i128::from(i16::deserialize(d)?),
        IntWidth::I32 => i128::from(i32::deserialize(d)?),
        IntWidth::I64 => i128::from(i64::deserialize(d)?),
        IntWidth::I128 => i128::deserialize(d)?,
    })
}

/// An `Option` field. A null is `None`, the struct wraps anything else in `Some`.
struct OptionalScalar(ScalarIr);

impl<'de> Visitor<'de> for OptionalScalar {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("option")
    }

    fn visit_none<E: Error>(self) -> Result<Value, E> {
        Ok(Value::none())
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::none())
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Scalar(self.0).deserialize(d)
    }
}
