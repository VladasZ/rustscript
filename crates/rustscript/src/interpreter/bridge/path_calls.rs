//! The path calls the front door hands out by id, plus the process, duration and datetime struct methods.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::{VArgs, arg};
use crate::interpreter::bytecode::{BuiltinId, MethodName, PathId};
use crate::interpreter::native::Native;
use crate::interpreter::shared::{self, parse_rfc3339};
use crate::interpreter::value::Value;

/// Runs before the range expands to its iterator value.
pub(super) fn range_builtin(
    recv: &Value,
    name: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    let Value::Range {
        start,
        end,
        inclusive,
    } = recv
    else {
        return Ok(None);
    };
    match name.id {
        BuiltinId::Clone => return Ok(Some(recv.clone())),
        BuiltinId::Contains => {
            let Some(Value::Int(value)) = args.first() else {
                bail!("range contains needs an integer");
            };
            return Ok(Some(Value::Bool(if *inclusive {
                *value >= *start && *value <= *end
            } else {
                *value >= *start && *value < *end
            })));
        }
        BuiltinId::Len | BuiltinId::Count => {
            let extra = i64::from(*inclusive && end >= start);
            return Ok(Some(Value::Int(end.saturating_sub(*start) + extra)));
        }
        BuiltinId::IsEmpty => {
            return Ok(Some(Value::Bool(if *inclusive {
                start > end
            } else {
                start >= end
            })));
        }
        _ => {}
    }
    Ok(None)
}

/// `usize::MAX`, `i32::MIN`, `f32::NAN` and friends, at their real width.
pub(super) fn numeric_limit(id: PathId) -> Option<Value> {
    use crate::interpreter::numeric::IntWidth;
    Some(match id {
        PathId::F64Epsilon => Value::Float(f64::EPSILON),
        PathId::F64Max => Value::Float(f64::MAX),
        PathId::F64Min => Value::Float(f64::MIN),
        PathId::F64MinPositive => Value::Float(f64::MIN_POSITIVE),
        PathId::F64Infinity => Value::Float(f64::INFINITY),
        PathId::F64NegInfinity => Value::Float(f64::NEG_INFINITY),
        PathId::F64Nan => Value::Float(f64::NAN),
        PathId::F32Epsilon => Value::F32(f32::EPSILON),
        PathId::F32Max => Value::F32(f32::MAX),
        PathId::F32Min => Value::F32(f32::MIN),
        PathId::F32MinPositive => Value::F32(f32::MIN_POSITIVE),
        PathId::F32Infinity => Value::F32(f32::INFINITY),
        PathId::F32NegInfinity => Value::F32(f32::NEG_INFINITY),
        PathId::F32Nan => Value::F32(f32::NAN),
        // u128 bounds are reinterpreted bits in `Value::Big`
        PathId::I128Max => Value::Big(i128::MAX, IntWidth::I128),
        PathId::I128Min => Value::Big(i128::MIN, IntWidth::I128),
        PathId::U128Max => Value::Big(u128::MAX.cast_signed(), IntWidth::U128),
        PathId::U128Min => Value::Big(0, IntWidth::U128),
        PathId::I8Max
        | PathId::I16Max
        | PathId::I32Max
        | PathId::I64Max
        | PathId::IsizeMax
        | PathId::U8Max
        | PathId::U16Max
        | PathId::U32Max
        | PathId::U64Max
        | PathId::UsizeMax => {
            let w = IntWidth::parse(id.namespace())?;
            Value::int_of_width(w.max(), w)
        }
        PathId::I8Min
        | PathId::I16Min
        | PathId::I32Min
        | PathId::I64Min
        | PathId::IsizeMin
        | PathId::U8Min
        | PathId::U16Min
        | PathId::U32Min
        | PathId::U64Min
        | PathId::UsizeMin => {
            let w = IntWidth::parse(id.namespace())?;
            Value::int_of_width(w.min(), w)
        }
        _ => return None,
    })
}

/// None when no bridge handles the id.
pub(super) fn bridge_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    match id {
        PathId::UtcNow | PathId::LocalNow => {
            return Ok(Some(now_datetime(id == PathId::LocalNow)));
        }
        PathId::DateTimeParseFromRfc3339 => {
            return Ok(Some(match parse_rfc3339(&arg(args, 0)?.display()) {
                Ok((unix_secs, nanos, offset)) => {
                    Value::ok(datetime_value(unix_secs, nanos, false, offset))
                }
                Err(e) => Value::err(Value::str(e)),
            }));
        }
        PathId::TimeSleep => return Ok(Some(sleep_future(args))),
        PathId::TaskYieldNow => return Ok(Some(yield_future())),
        PathId::ReqwestGet
        | PathId::ReqwestBlockingGet
        | PathId::ReqwestClientNew
        | PathId::ReqwestClientBuilder
        | PathId::ReqwestBlockingClientNew
        | PathId::ReqwestBlockingClientBuilder
        | PathId::RedirectPolicyNone
        | PathId::RedirectPolicyLimited => {
            return crate::interpreter::http::reqwest_call(id, args).map(Some);
        }
        _ => {}
    }
    if let Some(v) = crate::interpreter::ratatui::ratatui_assoc(id, args) {
        return Ok(Some(v));
    }
    if let Some(v) = crate::interpreter::std_bridge::native_call(id, args)? {
        return Ok(Some(v));
    }
    crate::interpreter::assoc::assoc_fn(id, args)
}

pub(super) fn exitstatus_method(
    s: &Arc<crate::interpreter::value::StructData>,
    name: &MethodName,
) -> Result<Value> {
    let m = name.id;
    let success = matches!(s.get("success"), Some(Value::Bool(true)));
    let code = match s.get("code") {
        Some(Value::Int(c)) => Some(c),
        _ => None,
    };
    match shared::exit_status_core(m, success, code) {
        Some(shared::ExitOut::Bool(b)) => Ok(Value::Bool(b)),
        Some(shared::ExitOut::OptInt(Some(c))) => Ok(Value::some(Value::Int(c))),
        Some(shared::ExitOut::OptInt(None)) => Ok(Value::none()),
        None => bail!("unknown method `{}` on ExitStatus", name.text),
    }
}

pub(super) fn output_method(
    s: &Arc<crate::interpreter::value::StructData>,
    name: &MethodName,
) -> Result<Value> {
    let m = name.id;
    Ok(match m {
        BuiltinId::Status | BuiltinId::Stdout | BuiltinId::Stderr => s
            .get(m.name())
            .ok_or_else(|| anyhow!("Output has no `{m}` field"))?,
        _ => bail!("unknown method `{}` on Output", name.text),
    })
}

pub(super) fn sleep_future(args: &[Value]) -> Value {
    let duration = args
        .first()
        .and_then(crate::interpreter::std_bridge::duration_from_value)
        .unwrap_or(Duration::ZERO);
    Native::Future(Box::pin(async move {
        tokio::time::sleep(duration).await;
        Value::Unit
    }))
    .wrap()
}

pub(super) fn yield_future() -> Value {
    Native::Future(Box::pin(async {
        tokio::task::yield_now().await;
        Value::Unit
    }))
    .wrap()
}

pub(super) fn datetime_value(secs: i64, nanos: u32, local: bool, offset: i32) -> Value {
    Value::struct_of(
        "DateTime",
        [
            ("secs".into(), Value::Int(secs)),
            ("nanos".into(), Value::Int(i64::from(nanos))),
            ("local".into(), Value::Bool(local)),
            ("offset".into(), Value::Int(i64::from(offset))),
        ],
    )
}

pub(super) fn now_datetime(local: bool) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    datetime_value(
        i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
        now.subsec_nanos(),
        local,
        0,
    )
}

pub(super) fn datetime_method(
    s: &Arc<crate::interpreter::value::StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let m = name.id;
    let secs = match s.get("secs") {
        Some(Value::Int(v)) => v,
        _ => 0,
    };
    let nanos = match s.get("nanos") {
        Some(Value::Int(v)) => u32::try_from(v).unwrap_or_default(),
        _ => 0,
    };
    let local = matches!(s.get("local"), Some(Value::Bool(true)));
    let offset = match s.get("offset") {
        Some(Value::Int(v)) => i32::try_from(v).unwrap_or_default(),
        _ => 0,
    };
    match shared::datetime_core(m, secs, nanos, local, offset, &VArgs(args)) {
        Some(shared::DateOut::Int(i)) => Ok(Value::Int(i)),
        Some(shared::DateOut::Text(t)) => Ok(Value::str(t)),
        None => bail!("unknown method `{}` on DateTime", name.text),
    }
}

pub(super) fn duration_method(
    s: &Arc<crate::interpreter::value::StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let m = name.id;
    let secs =
        u64::try_from(crate::interpreter::std_bridge::field_int(s, "secs")).unwrap_or_default();
    let nanos =
        u32::try_from(crate::interpreter::std_bridge::field_int(s, "nanos")).unwrap_or_default();
    if let BuiltinId::CheckedAdd | BuiltinId::CheckedSub = m {
        let own = Duration::new(secs, nanos);
        let Some(other) = args
            .first()
            .and_then(crate::interpreter::std_bridge::duration_from_value)
        else {
            bail!("`{}` on Duration takes a Duration argument", name.text);
        };
        let out = match m {
            BuiltinId::CheckedAdd => own.checked_add(other),
            _ => own.checked_sub(other),
        };
        return Ok(out.map_or_else(Value::none, |d| {
            Value::some(crate::interpreter::std_bridge::make_duration(d))
        }));
    }
    match shared::duration_core(m, secs, nanos) {
        Some(shared::DurOut::Int(i)) => Ok(Value::Int(i)),
        Some(shared::DurOut::Float(f)) => Ok(Value::Float(f)),
        Some(shared::DurOut::Bool(b)) => Ok(Value::Bool(b)),
        None => bail!("unknown method `{}` on Duration", name.text),
    }
}
