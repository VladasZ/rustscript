//! Bridge for the jsonwebtoken crate: Algorithm values, Header and
//! EncodingKey construction, and `encode` for signing tokens.

use std::str::FromStr;

use anyhow::{Result, bail};
use jsonwebtoken::{Algorithm, EncodingKey, Header};

use super::builtins::option_inner;
use super::json_bridge::value_to_json;
use super::std_bridge::bytes_arg;
use super::value::{StructData, Value};

/// Recognize `Algorithm::ES256` and friends used as a path value.
pub(super) fn jwt_algorithm(ty: &str, variant: &str) -> Option<Value> {
    if ty != "Algorithm" || Algorithm::from_str(variant).is_err() {
        return None;
    }
    Some(Value::Enum {
        enum_name: "Algorithm".into(),
        variant: variant.into(),
        data: Value::empty_data(),
    })
}

pub(super) fn jwt_assoc(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        ("Header", "new") | ("Header", "default") => {
            let alg = match args.first() {
                Some(v) => v.clone(),
                None => jwt_algorithm("Algorithm", "HS256").expect("HS256 is a known algorithm"),
            };
            // The shape carries every header field a script can set later,
            // since a shape cannot grow after the instance exists.
            Value::struct_of(
                "Header",
                [
                    ("alg".into(), alg),
                    ("typ".into(), Value::some(Value::str("JWT"))),
                    ("kid".into(), Value::none()),
                    ("cty".into(), Value::none()),
                ],
            )
        }
        ("EncodingKey", "from_secret") => key_value("secret", args),
        ("EncodingKey", "from_ec_pem") => {
            match EncodingKey::from_ec_pem(&bytes_arg(args.first())) {
                Ok(_) => Value::ok(key_value("ec_pem", args)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => return Ok(None),
    }))
}

/// The real `EncodingKey` is opaque, so the value keeps the constructor kind
/// and its raw input, and the key is rebuilt when `encode` runs.
fn key_value(kind: &str, args: &[Value]) -> Value {
    Value::struct_of(
        "EncodingKey",
        [
            ("kind".into(), Value::str(kind)),
            ("data".into(), args.first().cloned().unwrap_or(Value::Unit)),
        ],
    )
}

pub(super) fn jwt_encode(args: &[Value]) -> Result<Value> {
    let (Some(Value::Struct(header)), Some(claims), Some(Value::Struct(key))) =
        (args.first(), args.get(1), args.get(2))
    else {
        bail!("encode takes a header, claims, and an encoding key");
    };
    let mut real = Header::new(header_algorithm(header)?);
    real.typ = opt_string(header, "typ");
    real.kid = opt_string(header, "kid");
    real.cty = opt_string(header, "cty");

    let bytes = bytes_arg(key.get("data").as_ref());
    let kind = key.get("kind").map(|v| v.display()).unwrap_or_default();
    let real_key = match kind.as_str() {
        "secret" => EncodingKey::from_secret(&bytes),
        "ec_pem" => match EncodingKey::from_ec_pem(&bytes) {
            Ok(k) => k,
            Err(e) => return Ok(Value::err(Value::str(e.to_string()))),
        },
        other => bail!("`{other}` is not an EncodingKey"),
    };

    Ok(
        match jsonwebtoken::encode(&real, &value_to_json(claims)?, &real_key) {
            Ok(token) => Value::ok(Value::str(token)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
    )
}

fn header_algorithm(s: &StructData) -> Result<Algorithm> {
    let Some(Value::Enum { variant, .. }) = s.get("alg") else {
        bail!("the header has no algorithm");
    };
    match Algorithm::from_str(&variant) {
        Ok(a) => Ok(a),
        Err(_) => bail!("unknown JWT algorithm `{variant}`"),
    }
}

fn opt_string(s: &StructData, field: &str) -> Option<String> {
    option_inner(&s.get(field)?).map(|v| v.display())
}
