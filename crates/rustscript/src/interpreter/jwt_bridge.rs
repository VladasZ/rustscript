//! The jsonwebtoken bridge.

use std::str::FromStr;

use anyhow::{Result, bail};
use jsonwebtoken::{Algorithm, EncodingKey, Header};

use super::bridge::arg;
use super::bytecode::PathId;
use super::enum_def::ALGORITHM;
use super::iterator::option_inner;
use super::json_bridge::pvalue_to_json;
use super::native_methods::value_to_bytes;
use super::value::{StructData, Value};

pub(super) fn jwt_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::HeaderNew | PathId::HeaderDefault => {
            let alg = match args.first() {
                Some(v) => v.clone(),
                None => Value::enum_named(&ALGORITHM, "HS256", Vec::new())
                    .expect("HS256 is a known algorithm"),
            };
            // A shape cannot grow after the instance exists.
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
        PathId::EncodingKeyFromSecret => key_value("secret", args)?,
        PathId::EncodingKeyFromEcPem => {
            match EncodingKey::from_ec_pem(&value_to_bytes(args.first())) {
                Ok(_) => Value::ok(key_value("ec_pem", args)?),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => return Ok(None),
    }))
}

/// The real `EncodingKey` is opaque, so the key is rebuilt when `encode`
/// runs.
fn key_value(kind: &str, args: &[Value]) -> Result<Value> {
    Ok(Value::struct_of(
        "EncodingKey",
        [
            ("kind".into(), Value::str(kind)),
            ("data".into(), arg(args, 0)?),
        ],
    ))
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

    let bytes = value_to_bytes(key.get("data").as_ref());
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
        match jsonwebtoken::encode(&real, &pvalue_to_json(claims)?, &real_key) {
            Ok(token) => Value::ok(Value::str(token)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
    )
}

fn header_algorithm(s: &StructData) -> Result<Algorithm> {
    let Some(Value::Enum { def, variant, .. }) = s.get("alg") else {
        bail!("the header has no algorithm");
    };
    match Algorithm::from_str(def.variant_name(variant)) {
        Ok(a) => Ok(a),
        Err(_) => bail!("unknown JWT algorithm `{variant}`"),
    }
}

fn opt_string(s: &StructData, field: &str) -> Option<String> {
    option_inner(&s.get(field)?).map(|v| v.display())
}
