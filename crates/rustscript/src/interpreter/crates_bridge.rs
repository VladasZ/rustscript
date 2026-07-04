//! Bridges for the extra crates a script may use: base64, chrono,
//! rand and friends. Split from `builtins.rs`.


use anyhow::{Result, bail};

use super::native::Native;

use super::value::{StructData, Value};

use super::json_bridge::*;
use super::jwt_bridge::*;
use super::std_bridge::*;


/// `module::func` call is not a plain std bridge.
pub(super) fn crate_bridge(module: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    let s0 = || args.first().map(|v| v.display()).unwrap_or_default();
    Ok(Some(match (module, func) {
        // dirs -------------------------------------------------------------
        ("dirs", "home_dir") => opt_path(dirs::home_dir()),
        ("dirs", "cache_dir") => opt_path(dirs::cache_dir()),
        ("dirs", "config_dir") => opt_path(dirs::config_dir()),
        ("dirs", "config_local_dir") => opt_path(dirs::config_local_dir()),
        ("dirs", "data_dir") => opt_path(dirs::data_dir()),
        ("dirs", "data_local_dir") => opt_path(dirs::data_local_dir()),
        ("dirs", "executable_dir") => opt_path(dirs::executable_dir()),
        ("dirs", "runtime_dir") => opt_path(dirs::runtime_dir()),
        ("dirs", "desktop_dir") => opt_path(dirs::desktop_dir()),
        ("dirs", "download_dir") => opt_path(dirs::download_dir()),
        ("dirs", "document_dir") => opt_path(dirs::document_dir()),
        // which ------------------------------------------------------------
        ("which", "which") => match which::which(s0()) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // glob -------------------------------------------------------------
        ("glob", "glob") => match glob::glob(&s0()) {
            Ok(paths) => Value::ok(Value::vec(
                paths
                    .map(|r| match r {
                        Ok(p) => Value::ok(make_path(p.display().to_string())),
                        Err(e) => Value::err(Value::str(e.to_string())),
                    })
                    .collect(),
            )),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // hex --------------------------------------------------------------
        ("hex", "encode") => Value::str(hex::encode(bytes_arg(args.first()))),
        ("hex", "decode") => match hex::decode(s0()) {
            Ok(b) => Value::ok(bytes_to_vec(&b)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // toml -------------------------------------------------------------
        ("toml", "from_str") => match toml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_value(j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("toml", "to_string") | ("toml", "to_string_pretty") => {
            match toml::to_string(&value_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // serde_yaml -------------------------------------------------------
        ("serde_yaml", "from_str") => match serde_yaml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_value(j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("serde_yaml", "to_string") => {
            match serde_yaml::to_string(&value_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // rand -------------------------------------------------------------
        ("rand", "rng") | ("rand", "thread_rng") => Value::struct_of("Rng", []),
        ("rand", "random") => Value::Float(rand::random::<f64>()),
        // chrono -----------------------------------------------------------
        ("Utc", "now") | ("Local", "now") => now_datetime(module == "Local"),
        // jsonwebtoken -------------------------------------------------------
        ("jsonwebtoken", "encode") => jwt_encode(args)?,
        // tempfile ---------------------------------------------------------
        ("tempfile", "tempdir") => match tempfile::tempdir() {
            Ok(d) => Value::ok(Native::TempDir(d).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("tempfile", "tempfile") => match tempfile::tempfile() {
            Ok(f) => Value::ok(Native::File(std::io::BufReader::new(f)).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        _ => return Ok(None),
    }))
}

/// Recognize a base64 engine constant name and build a marker value carrying
/// which alphabet it uses, so `.encode`/`.decode` can pick the right engine.
pub(super) fn base64_engine(name: &str) -> Option<Value> {
    let kind = match name {
        "STANDARD" | "BASE64_STANDARD" => "standard",
        "STANDARD_NO_PAD" | "BASE64_STANDARD_NO_PAD" => "standard_no_pad",
        "URL_SAFE" | "BASE64_URL_SAFE" => "url_safe",
        "URL_SAFE_NO_PAD" | "BASE64_URL_SAFE_NO_PAD" => "url_safe_no_pad",
        _ => return None,
    };
    Some(Value::struct_of("Base64Engine", [("kind".into(), Value::str(kind))]))
}

pub(super) fn base64_method(s: &StructData, method: &str, args: &[Value]) -> Result<Value> {
    use base64::Engine;
    use base64::engine::general_purpose::{
        STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
    };
    let kind = s.get("kind").map(|v| v.display()).unwrap_or_default();
    macro_rules! pick {
        ($m:ident, $($a:tt)*) => {
            match kind.as_str() {
                "standard_no_pad" => STANDARD_NO_PAD.$m($($a)*),
                "url_safe" => URL_SAFE.$m($($a)*),
                "url_safe_no_pad" => URL_SAFE_NO_PAD.$m($($a)*),
                _ => STANDARD.$m($($a)*),
            }
        };
    }
    Ok(match method {
        "encode" => Value::str(pick!(encode, bytes_arg(args.first()))),
        "decode" => {
            let input = args.first().map(|v| v.display()).unwrap_or_default();
            match pick!(decode, &input) {
                Ok(b) => Value::ok(bytes_to_vec(&b)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => bail!("unknown method `{method}` on a base64 engine"),
    })
}

/// Build a `DateTime` value for `Utc::now()` / `Local::now()`, storing the unix
/// timestamp so `format` can reconstruct a real chrono value.
pub(super) fn now_datetime(local: bool) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Value::struct_of(
        "DateTime",
        [
            ("secs".into(), Value::Int(now.as_secs() as i64)),
            ("nanos".into(), Value::Int(now.subsec_nanos() as i64)),
            ("local".into(), Value::Bool(local)),
        ],
    )
}

pub(super) fn datetime_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
    use chrono::{DateTime, Local, Utc};
    let secs = field_int(s, "secs");
    let nanos = field_int(s, "nanos") as u32;
    let local = matches!(s.get("local"), Some(Value::Bool(true)));
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
    Ok(match name {
        "timestamp" => Value::Int(secs as i64),
        "timestamp_millis" => Value::Int(secs as i64 * 1000 + (nanos / 1_000_000) as i64),
        "to_rfc3339" => Value::str(utc.to_rfc3339()),
        "format" => {
            let fmt = args.first().map(|v| v.display()).unwrap_or_default();
            if local {
                Value::str(utc.with_timezone(&Local).format(&fmt).to_string())
            } else {
                Value::str(utc.format(&fmt).to_string())
            }
        }
        "year" => Value::Int(chrono::Datelike::year(&utc) as i64),
        "month" => Value::Int(chrono::Datelike::month(&utc) as i64),
        "day" => Value::Int(chrono::Datelike::day(&utc) as i64),
        "hour" => Value::Int(chrono::Timelike::hour(&utc) as i64),
        "minute" => Value::Int(chrono::Timelike::minute(&utc) as i64),
        "second" => Value::Int(chrono::Timelike::second(&utc) as i64),
        _ => bail!("unknown method `{name}` on DateTime"),
    })
}

pub(super) fn rng_method(name: &str, args: &[Value]) -> Result<Value> {
    use rand::RngExt;
    let mut rng = rand::rng();
    Ok(match name {
        "random_range" | "gen_range" => match args.first() {
            Some(Value::Range { start, end, inclusive }) => {
                let hi = if *inclusive { end + 1 } else { *end };
                if hi > *start {
                    Value::Int(rng.random_range(*start..hi))
                } else {
                    Value::Int(*start)
                }
            }
            _ => bail!("random_range needs a range"),
        },
        "random_bool" | "gen_bool" => {
            let p = match args.first() {
                Some(Value::Float(f)) => *f,
                Some(Value::Int(i)) => *i as f64,
                _ => 0.5,
            };
            Value::Bool(rng.random_bool(p.clamp(0.0, 1.0)))
        }
        "random" | "r#gen" | "gen" => Value::Float(rng.random::<f64>()),
        "fill_bytes" | "fill" => {
            if let Some(Value::Vec(v)) = args.first() {
                let mut buf = v.borrow_mut();
                for slot in buf.iter_mut() {
                    *slot = Value::Int(rng.random::<u8>() as i64);
                }
            }
            Value::Unit
        }
        _ => bail!("unknown method `{name}` on Rng"),
    })
}
