//! Width aware dispatch for integer, float and scalar methods.

use num_traits::AsPrimitive;

use anyhow::{Result, bail};

use super::VArgs;
use crate::interpreter::bytecode::{BuiltinId, MethodName};
use crate::interpreter::methods::{self, make_ordering};
use crate::interpreter::shared::{self, CharOut, F32Out, Num, NumOut};
use crate::interpreter::value::Value;

pub(in crate::interpreter) fn int_method(
    recv: &Value,
    name: &MethodName,
    args: &[Value],
) -> Option<Result<Value>> {
    let m = name.id;
    // An operand with no i128 image is a u128 past `i128::MAX`. Checking through the `int_parts`
    // failure keeps this off the hot path.
    let Some((value, mut width)) = recv.int_parts() else {
        return big_int_route(recv, name, args);
    };
    // no method takes more than 3 integers, so the decoded arguments live on the stack
    let mut buffer = [0i128; 4];
    let mut spill = Vec::new();
    let mut count = 0usize;
    for arg in args {
        let Some((arg_value, arg_width)) = arg.int_parts() else {
            return big_int_route(recv, name, args);
        };
        if count < buffer.len() {
            buffer[count] = arg_value;
        } else {
            spill.push(arg_value);
        }
        count += 1;
        // Receiver and argument have 1 type, so either width works for both. A shift amount's u32
        // must not redefine the receiver.
        if !crate::interpreter::int_methods::takes_amount_arg(m)
            && let Ok(unified) = crate::interpreter::numeric::unify(width, arg_width)
        {
            width = unified;
        }
    }
    let decoded: &[i128] = if count <= buffer.len() {
        &buffer[..count]
    } else {
        spill.splice(0..0, buffer);
        &spill
    };
    // `value` is already the raw bit pattern the native cores take
    if width.is_big() {
        let out = crate::interpreter::int_methods::big_int_method(m, width, value, decoded)?;
        return Some(out.map(|o| int_out(o, width)));
    }
    Some(
        match crate::interpreter::int_methods::int_method(m, width, value, decoded)? {
            Ok(out) => Ok(int_out(out, width)),
            Err(error) => Err(error),
        },
    )
}

/// `None` when no 128 bit operand is present. Cold, only reached through the `int_parts` failure path.
#[cold]
pub(super) fn big_int_route(
    recv: &Value,
    name: &MethodName,
    args: &[Value],
) -> Option<Result<Value>> {
    let m = name.id;
    let mut width = match recv {
        Value::Big(_, w) => Some(*w),
        _ => None,
    };
    if width.is_none() {
        width = args.iter().find_map(|v| match v {
            Value::Big(_, w) => Some(*w),
            _ => None,
        });
    }
    let width = width?;
    let bits = big_bits(recv)?;
    let decoded: Option<Vec<i128>> = args.iter().map(big_bits).collect();
    let out = crate::interpreter::int_methods::big_int_method(m, width, bits, &decoded?)?;
    Some(out.map(|o| int_out(o, width)))
}

/// A `Big` carries the bits directly, anything else by its value, which is the same pattern for
/// everything valid Rust can mix with it.
pub(super) fn big_bits(v: &Value) -> Option<i128> {
    match v {
        Value::Big(bits, _) => Some(*bits),
        Value::Int(i) => Some(i128::from(*i)),
        other => other.int_parts().map(|(value, _)| value),
    }
}

/// Called before `bridge_image` widens the receiver, so the result keeps the f32 tag.
pub(super) fn f32_method(recv: f32, name: BuiltinId, args: &[Value]) -> Result<Option<Value>> {
    Ok(
        shared::f32_core(recv, name, &VArgs(args))?.map(|out| match out {
            F32Out::Val(value) => Value::F32(value),
            F32Out::Bool(value) => Value::Bool(value),
            F32Out::Bytes(bytes) => Value::vec(
                bytes
                    .into_iter()
                    .map(|byte| Value::Int(i64::from(byte)))
                    .collect(),
            ),
            F32Out::Ordering(ordering) => make_ordering(ordering),
            F32Out::SomeOrdering(ordering) => Value::some(make_ordering(ordering)),
        }),
    )
}

pub(super) fn int_out(
    out: crate::interpreter::int_methods::IntOut,
    width: crate::interpreter::numeric::IntWidth,
) -> Value {
    use crate::interpreter::int_methods::IntOut;
    match out {
        IntOut::Same(value) => Value::int_of_width(value, width),
        // counts are u32, otherwise `!x.count_ones()` prints -1 instead of 4294967295
        IntOut::Count(count) => Value::int_of_width(
            i128::from(count),
            crate::interpreter::numeric::IntWidth::U32,
        ),
        IntOut::Bool(value) => Value::Bool(value),
        IntOut::Checked(Some(value)) => Value::some(Value::int_of_width(value, width)),
        IntOut::Checked(None) | IntOut::CheckedCount(None) => Value::none(),
        IntOut::SomeFloat(value) => Value::some(Value::Float(value)),
        IntOut::Ordering(ordering) => make_ordering(ordering),
        IntOut::Bytes(bytes) => Value::vec(
            bytes
                .into_iter()
                .map(|byte| Value::Int(i64::from(byte)))
                .collect(),
        ),
        IntOut::Overflowing(value, wrapped) => Value::tuple(vec![
            Value::int_of_width(value, width),
            Value::Bool(wrapped),
        ]),
        IntOut::CheckedCount(Some(count)) => Value::some(Value::int_of_width(
            i128::from(count),
            crate::interpreter::numeric::IntWidth::U32,
        )),
    }
}

pub(super) fn scalar_method(recv: &Value, name: &MethodName, args: &[Value]) -> Result<Value> {
    let m = name.id;
    // a conversion that only changes the static type is a no-op on a scalar
    match m {
        BuiltinId::ToString => return Ok(Value::str(recv.display())),
        BuiltinId::Clone | BuiltinId::Into => return Ok(recv.clone()),
        _ => {}
    }
    // serde accessors on a decoded scalar, a wrong type accessor is None like in serde
    if matches!(
        m,
        BuiltinId::AsStr
            | BuiltinId::AsI64
            | BuiltinId::AsU64
            | BuiltinId::AsF64
            | BuiltinId::AsBool
            | BuiltinId::AsArray
            | BuiltinId::AsArrayMut
            | BuiltinId::AsObject
            | BuiltinId::AsObjectMut
    ) {
        let matched = match (recv, m) {
            (Value::Bool(_), BuiltinId::AsBool)
            | (Value::Str(_), BuiltinId::AsStr)
            | (Value::Int(_) | Value::IntW(..), BuiltinId::AsI64 | BuiltinId::AsU64)
            | (Value::Float(_), BuiltinId::AsF64) => true,
            (Value::Int(i), BuiltinId::AsF64) => {
                return Ok(Value::some(Value::Float(AsPrimitive::<f64>::as_(*i))));
            }
            _ => false,
        };
        return Ok(if matched {
            Value::some(recv.clone())
        } else {
            Value::none()
        });
    }
    let n = match recv {
        Value::Int(i) => Some(Num::Int(*i)),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    };
    if let Some(n) = n {
        if let Some(out) = shared::num_core(n, m, &VArgs(args))? {
            return Ok(num_out(out));
        }
        bail!("unknown method `{}` on a number", name.text);
    }
    if let Value::Char(ch) = recv
        && let Some(out) = shared::char_method(*ch, m, &VArgs(args))
    {
        return Ok(match out? {
            CharOut::Bool(v) => Value::Bool(v),
            CharOut::Char(c) => Value::Char(c),
            CharOut::Str(s) => Value::str(s),
            CharOut::OptU32(Some(digit)) => Value::some(Value::int_of_width(
                i128::from(digit),
                crate::interpreter::numeric::IntWidth::U32,
            )),
            CharOut::OptU32(None) => Value::none(),
            CharOut::USize(n) => crate::interpreter::shared::usize_value(n),
        });
    }
    methods::generic_method(recv, name, args)
}

pub(super) fn num_out(out: NumOut) -> Value {
    match out {
        NumOut::Int(i) => Value::Int(i),
        NumOut::Float(f) => Value::Float(f),
        NumOut::Bool(b) => Value::Bool(b),
        NumOut::Bytes(bytes) => Value::vec(
            bytes
                .into_iter()
                .map(|byte| Value::Int(i64::from(byte)))
                .collect(),
        ),
        NumOut::SomeInt(i) => Value::some(Value::Int(i)),
        NumOut::SomeFloat(f) => Value::some(Value::Float(f)),
        NumOut::Nothing => Value::none(),
        NumOut::Ordering(o) => make_ordering(o),
        NumOut::SomeOrdering(o) => Value::some(make_ordering(o)),
    }
}
