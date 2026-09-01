//! The `Duration` and `DateTime` method cores.

use num_traits::AsPrimitive;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::Args;
use crate::interpreter::bytecode::{BinKind, BuiltinId};

pub(crate) enum DurOut {
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// The checked std ops with the real panic messages.
pub(crate) fn duration_arith(op: BinKind, a: Duration, b: Duration) -> Result<Duration> {
    match op {
        BinKind::Add => a
            .checked_add(b)
            .ok_or_else(|| anyhow!("overflow when adding durations")),
        BinKind::Sub => a
            .checked_sub(b)
            .ok_or_else(|| anyhow!("overflow when subtracting durations")),
        _ => bail!("cannot apply that operator to two durations"),
    }
}

pub(crate) fn duration_core(name: BuiltinId, secs: u64, nanos: u32) -> Option<DurOut> {
    let total = u128::from(secs) * 1_000_000_000 + u128::from(nanos);
    Some(match name {
        BuiltinId::AsSecs => DurOut::Int(i64::try_from(secs).unwrap_or(i64::MAX)),
        BuiltinId::AsMillis => DurOut::Int(i64::try_from(total / 1_000_000).unwrap_or(i64::MAX)),
        BuiltinId::AsMicros => DurOut::Int(i64::try_from(total / 1_000).unwrap_or(i64::MAX)),
        BuiltinId::AsNanos => DurOut::Int(i64::try_from(total).unwrap_or(i64::MAX)),
        BuiltinId::SubsecNanos => DurOut::Int(i64::from(nanos)),
        BuiltinId::SubsecMillis => DurOut::Int(i64::from(nanos / 1_000_000)),
        BuiltinId::SubsecMicros => DurOut::Int(i64::from(nanos / 1_000)),
        BuiltinId::AsSecsF64 => {
            DurOut::Float(AsPrimitive::<f64>::as_(secs) + f64::from(nanos) / 1e9)
        }
        BuiltinId::IsZero => DurOut::Bool(total == 0),
        _ => return None,
    })
}

// datetime

pub(crate) enum DateOut {
    Int(i64),
    Text(String),
}

/// `parse_from_rfc3339` reduced to unix seconds, nanos and the offset. The error is the real
/// chrono message.
pub(crate) fn parse_rfc3339(text: &str) -> Result<(i64, u32, i32), String> {
    use chrono::{DateTime, Offset, Timelike};
    match DateTime::parse_from_rfc3339(text) {
        Ok(dt) => Ok((
            dt.timestamp(),
            dt.nanosecond(),
            dt.offset().fix().local_minus_utc(),
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// `local` picks the machine timezone, otherwise the value is read through `offset`. A calendar field
/// is read in the zone the value carries, like in real chrono.
pub(crate) fn datetime_core(
    name: BuiltinId,
    secs: i64,
    nanos: u32,
    local: bool,
    offset: i32,
    args: &impl Args,
) -> Option<DateOut> {
    use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, Timelike, Utc};
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
    let view = if local {
        utc.with_timezone(&Local).fixed_offset()
    } else {
        utc.with_timezone(&FixedOffset::east_opt(offset).unwrap_or(Utc.fix()))
    };
    Some(match name {
        BuiltinId::Timestamp => DateOut::Int(secs),
        BuiltinId::TimestampMillis => DateOut::Int(secs * 1000 + i64::from(nanos / 1_000_000)),
        BuiltinId::ToRfc3339 => DateOut::Text(view.to_rfc3339()),
        BuiltinId::Format => DateOut::Text(view.format(&args.text(0)).to_string()),
        BuiltinId::Year => DateOut::Int(i64::from(view.year())),
        BuiltinId::Month => DateOut::Int(i64::from(view.month())),
        BuiltinId::Day => DateOut::Int(i64::from(view.day())),
        BuiltinId::Hour => DateOut::Int(i64::from(view.hour())),
        BuiltinId::Minute => DateOut::Int(i64::from(view.minute())),
        BuiltinId::Second => DateOut::Int(i64::from(view.second())),
        _ => return None,
    })
}
