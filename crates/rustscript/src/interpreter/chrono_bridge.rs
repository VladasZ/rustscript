//! The chrono bridge beyond the `DateTime` core in `shared.rs`. Naive dates, timezone
//! conversion, weekdays, and signed `Duration` deltas. A naive value carries no zone, so it is
//! its own struct and only `and_utc` turns it into a `DateTime`.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike};

use super::bridge::arg;
use super::bytecode::{BuiltinId, MethodName, PathId};
use super::native::Native;
use super::value::{StructData, Value};

const NANOS_PER_SEC: i64 = 1_000_000_000;

fn field_int(s: &Arc<StructData>, name: &str) -> i64 {
    match s.get(name) {
        Some(Value::Int(v)) => v,
        _ => 0,
    }
}

/// The same shape `datetime_value` in `path_calls` builds.
fn datetime_struct(secs: i64, nanos: u32, local: bool, offset: i32) -> Value {
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

fn naive_date_struct(date: NaiveDate) -> Value {
    Value::struct_of(
        "NaiveDate",
        [(
            "days".into(),
            Value::Int(i64::from(date.num_days_from_ce())),
        )],
    )
}

fn naive_datetime_struct(dt: NaiveDateTime) -> Value {
    Value::struct_of(
        "NaiveDateTime",
        [
            ("secs".into(), Value::Int(dt.and_utc().timestamp())),
            (
                "nanos".into(),
                Value::Int(i64::from(dt.and_utc().timestamp_subsec_nanos())),
            ),
        ],
    )
}

/// Total signed nanoseconds, enough for around 292 years of delta.
fn delta_struct(nanos: i64) -> Value {
    Value::struct_of("TimeDelta", [("nanos".into(), Value::Int(nanos))])
}

/// `Utc` and `Local` written as values, the argument of `with_timezone`.
fn tz_struct(local: bool) -> Value {
    Value::struct_of("Tz", [("local".into(), Value::Bool(local))])
}

/// Panics on overflow exactly where chrono panics building the same duration.
fn delta_ctor(id: PathId, n: i64) -> Result<Value> {
    let factor: i64 = match id {
        PathId::DurationNanoseconds => 1,
        PathId::DurationMicroseconds => 1_000,
        PathId::DurationMilliseconds => 1_000_000,
        PathId::DurationSeconds => NANOS_PER_SEC,
        PathId::DurationMinutes => 60 * NANOS_PER_SEC,
        PathId::DurationHours => 3_600 * NANOS_PER_SEC,
        PathId::DurationDays => 86_400 * NANOS_PER_SEC,
        PathId::DurationWeeks => 7 * 86_400 * NANOS_PER_SEC,
        _ => bail!("`{id}` is not a Duration constructor"),
    };
    let nanos = n
        .checked_mul(factor)
        .ok_or_else(|| anyhow!("Duration overflows in nanoseconds: {n}"))?;
    Ok(delta_struct(nanos))
}

fn parse_error(e: chrono::ParseError) -> Value {
    Value::err(
        Native::ParseErr {
            display: e.to_string(),
            debug: format!("{e:?}"),
        }
        .wrap(),
    )
}

/// The chrono path calls. `None` when the id is not chrono's.
pub(super) fn chrono_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    let text = |i: usize| -> Result<String> {
        args.get(i)
            .map(Value::display)
            .ok_or_else(|| anyhow!("missing argument {i} for {id}"))
    };
    let int = |i: usize| -> Result<i64> {
        match args.get(i) {
            Some(v) => super::ops::int_of(v),
            None => bail!("missing argument {i} for {id}"),
        }
    };
    Ok(Some(match id {
        PathId::NaiveDateParseFromStr => match NaiveDate::parse_from_str(&text(0)?, &text(1)?) {
            Ok(d) => Value::ok(naive_date_struct(d)),
            Err(e) => parse_error(e),
        },
        PathId::NaiveDateTimeParseFromStr => {
            match NaiveDateTime::parse_from_str(&text(0)?, &text(1)?) {
                Ok(dt) => Value::ok(naive_datetime_struct(dt)),
                Err(e) => parse_error(e),
            }
        }
        PathId::DateTimeFromTimestamp => {
            let secs = int(0)?;
            let nanos = u32::try_from(int(1)?).ok();
            match nanos.and_then(|n| DateTime::from_timestamp(secs, n)) {
                Some(dt) => Value::some(datetime_struct(
                    dt.timestamp(),
                    dt.timestamp_subsec_nanos(),
                    false,
                    0,
                )),
                None => Value::none(),
            }
        }
        PathId::DurationNanoseconds
        | PathId::DurationMicroseconds
        | PathId::DurationMilliseconds
        | PathId::DurationSeconds
        | PathId::DurationMinutes
        | PathId::DurationHours
        | PathId::DurationDays
        | PathId::DurationWeeks => delta_ctor(id, int(0)?)?,
        _ => return Ok(None),
    }))
}

/// The `Utc` and `Local` markers as path values.
pub(super) fn chrono_const(id: PathId) -> Option<Value> {
    match id {
        PathId::Utc => Some(tz_struct(false)),
        PathId::Local => Some(tz_struct(true)),
        _ => None,
    }
}

pub(super) fn naive_date_method(
    s: &Arc<StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let date = NaiveDate::from_num_days_from_ce_opt(
        i32::try_from(field_int(s, "days")).unwrap_or_default(),
    )
    .unwrap_or_default();
    Ok(match name.id {
        BuiltinId::AndHmsOpt => {
            let part = |i: usize| {
                args.get(i)
                    .and_then(|v| super::ops::int_of(v).ok())
                    .and_then(|n| u32::try_from(n).ok())
            };
            let (Some(h), Some(m), Some(sec)) = (part(0), part(1), part(2)) else {
                bail!("and_hms_opt takes 3 integers");
            };
            match date.and_hms_opt(h, m, sec) {
                Some(dt) => Value::some(naive_datetime_struct(dt)),
                None => Value::none(),
            }
        }
        BuiltinId::Format => Value::str(date.format(&arg(args, 0)?.display()).to_string()),
        BuiltinId::Year => Value::Int(i64::from(date.year())),
        BuiltinId::Month => Value::Int(i64::from(date.month())),
        BuiltinId::Day => Value::Int(i64::from(date.day())),
        BuiltinId::Weekday => weekday_struct(date.weekday()),
        _ => bail!("unknown method `{name}` on NaiveDate"),
    })
}

pub(super) fn naive_datetime_method(s: &Arc<StructData>, name: &MethodName) -> Result<Value> {
    let secs = field_int(s, "secs");
    let nanos = u32::try_from(field_int(s, "nanos")).unwrap_or_default();
    let dt = DateTime::from_timestamp(secs, nanos)
        .unwrap_or_default()
        .naive_utc();
    Ok(match name.id {
        BuiltinId::AndUtc => datetime_struct(secs, nanos, false, 0),
        BuiltinId::Hour => Value::Int(i64::from(dt.hour())),
        BuiltinId::Minute => Value::Int(i64::from(dt.minute())),
        BuiltinId::Second => Value::Int(i64::from(dt.second())),
        BuiltinId::Weekday => weekday_struct(dt.weekday()),
        _ => bail!("unknown method `{name}` on NaiveDateTime"),
    })
}

fn weekday_struct(day: chrono::Weekday) -> Value {
    Value::struct_of(
        "Weekday",
        [(
            "from_monday".into(),
            Value::Int(i64::from(day.num_days_from_monday())),
        )],
    )
}

pub(super) fn timedelta_method(s: &Arc<StructData>, name: &MethodName) -> Result<Value> {
    let nanos = field_int(s, "nanos");
    Ok(match name.id {
        BuiltinId::NumNanoseconds => Value::some(Value::Int(nanos)),
        BuiltinId::NumMilliseconds => Value::Int(nanos / 1_000_000),
        BuiltinId::NumSeconds => Value::Int(nanos / NANOS_PER_SEC),
        BuiltinId::NumMinutes => Value::Int(nanos / (60 * NANOS_PER_SEC)),
        BuiltinId::NumHours => Value::Int(nanos / (3_600 * NANOS_PER_SEC)),
        BuiltinId::NumDays => Value::Int(nanos / (86_400 * NANOS_PER_SEC)),
        _ => bail!("unknown method `{name}` on Duration"),
    })
}

pub(super) fn weekday_method(s: &Arc<StructData>, name: &MethodName) -> Result<Value> {
    let from_monday = field_int(s, "from_monday");
    Ok(match name.id {
        BuiltinId::NumDaysFromMonday => Value::Int(from_monday),
        BuiltinId::NumDaysFromSunday => Value::Int((from_monday + 1) % 7),
        _ => bail!("unknown method `{name}` on Weekday"),
    })
}

/// The `DateTime` methods past the core in `shared.rs`. `None` hands back to the caller's error.
pub(super) fn datetime_extra(
    s: &Arc<StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    let secs = field_int(s, "secs");
    let nanos = u32::try_from(field_int(s, "nanos")).unwrap_or_default();
    Ok(Some(match name.id {
        BuiltinId::WithTimezone => {
            let local = match arg(args, 0)? {
                Value::Struct(tz) if &**tz.name() == "Tz" => {
                    matches!(tz.get("local"), Some(Value::Bool(true)))
                }
                other => bail!(
                    "with_timezone takes &Utc or &Local, got {}",
                    other.type_name()
                ),
            };
            datetime_struct(secs, nanos, local, 0)
        }
        BuiltinId::Weekday => {
            let dt = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
            weekday_struct(view_of(s, dt).weekday())
        }
        _ => return Ok(None),
    }))
}

/// The same viewing rule as `datetime_core`, weekday depends on the zone.
fn view_of(s: &Arc<StructData>, utc: DateTime<chrono::Utc>) -> DateTime<chrono::FixedOffset> {
    use chrono::{FixedOffset, Local, Offset, Utc};
    if matches!(s.get("local"), Some(Value::Bool(true))) {
        utc.with_timezone(&Local).fixed_offset()
    } else {
        let offset = i32::try_from(field_int(s, "offset")).unwrap_or_default();
        utc.with_timezone(&FixedOffset::east_opt(offset).unwrap_or(Utc.fix()))
    }
}

/// `DateTime` plus or minus a delta, and delta arithmetic. `None` when the shapes are not
/// chrono's, the generic op then reports its own error.
pub(super) fn chrono_arith(
    op: super::bytecode::BinKind,
    l: &Value,
    r: &Value,
) -> Option<Result<Value>> {
    use super::bytecode::BinKind;
    let named = |v: &Value, name: &str| -> Option<Arc<StructData>> {
        match v {
            Value::Struct(s) if &**s.name() == name => Some(s.clone()),
            _ => None,
        }
    };
    if let (Some(a), Some(b)) = (named(l, "TimeDelta"), named(r, "TimeDelta")) {
        let (x, y) = (field_int(&a, "nanos"), field_int(&b, "nanos"));
        let out = match op {
            BinKind::Add => x.checked_add(y),
            BinKind::Sub => x.checked_sub(y),
            _ => return None,
        };
        return Some(
            out.map(delta_struct)
                .ok_or_else(|| anyhow!("Duration arithmetic overflows")),
        );
    }
    let (dt, delta) = (named(l, "DateTime"), named(r, "TimeDelta"));
    let (Some(dt), Some(delta)) = (dt, delta) else {
        return None;
    };
    let total = i128::from(field_int(&dt, "secs")) * i128::from(NANOS_PER_SEC)
        + i128::from(field_int(&dt, "nanos"));
    let shifted = match op {
        BinKind::Add => total + i128::from(field_int(&delta, "nanos")),
        BinKind::Sub => total - i128::from(field_int(&delta, "nanos")),
        _ => return None,
    };
    let secs = i64::try_from(shifted.div_euclid(i128::from(NANOS_PER_SEC)));
    let nanos = u32::try_from(shifted.rem_euclid(i128::from(NANOS_PER_SEC)));
    let (Ok(secs), Ok(nanos)) = (secs, nanos) else {
        return Some(Err(anyhow!("DateTime arithmetic out of range")));
    };
    let local = matches!(dt.get("local"), Some(Value::Bool(true)));
    let offset = i32::try_from(field_int(&dt, "offset")).unwrap_or_default();
    Some(Ok(datetime_struct(secs, nanos, local, offset)))
}
