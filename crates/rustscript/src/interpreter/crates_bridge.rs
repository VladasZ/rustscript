//! Bridges for the extra crates a script may use: base64, chrono, rand, sha2,
//! hex, toml, yaml, glob, dirs, tempfile and friends.

use num_traits::AsPrimitive;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId as B, MethodName, PathId as P};
use super::json_bridge::{json_to_pvalue, pvalue_to_json};
use super::native::Native;
use super::native_methods::value_to_bytes;
use super::std_bridge::make_path;
use super::value::{StructData, Value};

fn opt_path(p: Option<std::path::PathBuf>) -> Value {
    match p {
        Some(p) => Value::some(make_path(p.display().to_string())),
        None => Value::none(),
    }
}

pub(super) fn bytes_to_vec(b: &[u8]) -> Value {
    Value::vec(b.iter().map(|x| Value::Int(i64::from(*x))).collect())
}

/// `module::func` call that is not a plain std bridge.
pub(super) fn crate_bridge(id: P, args: &[Value]) -> Result<Option<Value>> {
    let s0 = || args.first().map(Value::display).unwrap_or_default();
    Ok(Some(match id {
        // dirs -------------------------------------------------------------
        P::DirsHomeDir => opt_path(dirs::home_dir()),
        P::DirsCacheDir => opt_path(dirs::cache_dir()),
        P::DirsConfigDir => opt_path(dirs::config_dir()),
        P::DirsConfigLocalDir => opt_path(dirs::config_local_dir()),
        P::DirsDataDir => opt_path(dirs::data_dir()),
        P::DirsDataLocalDir => opt_path(dirs::data_local_dir()),
        P::DirsExecutableDir => opt_path(dirs::executable_dir()),
        P::DirsRuntimeDir => opt_path(dirs::runtime_dir()),
        P::DirsDesktopDir => opt_path(dirs::desktop_dir()),
        P::DirsDownloadDir => opt_path(dirs::download_dir()),
        P::DirsDocumentDir => opt_path(dirs::document_dir()),
        // which ------------------------------------------------------------
        P::WhichWhich => match which::which(s0()) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // glob -------------------------------------------------------------
        P::GlobGlob => match glob::glob(&s0()) {
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
        // sha2 -------------------------------------------------------------
        P::Sha256New | P::Sha256Default => {
            use sha2::Digest;
            Native::Sha256(sha2::Sha256::new()).wrap()
        }
        P::Sha256Digest => {
            use sha2::Digest;
            bytes_to_vec(&sha2::Sha256::digest(value_to_bytes(args.first())))
        }
        // regex free functions ---------------------------------------------
        P::RegexEscape => Value::str(regex::escape(&s0())),
        // hex --------------------------------------------------------------
        P::HexEncode => Value::str(hex::encode(value_to_bytes(args.first()))),
        P::HexDecode => match hex::decode(s0()) {
            Ok(b) => Value::ok(bytes_to_vec(&b)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // toml -------------------------------------------------------------
        P::TomlFromStr => match toml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_pvalue(j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        P::TomlToString | P::TomlToStringPretty => {
            match toml::to_string(&pvalue_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // serde_yaml -------------------------------------------------------
        P::SerdeYamlFromStr => match serde_yaml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_pvalue(j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        P::SerdeYamlToString => {
            match serde_yaml::to_string(&pvalue_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // rand -------------------------------------------------------------
        P::RandRng | P::RandThreadRng => Value::struct_of("Rng", []),
        P::RandRandom => Value::Float(rand::random::<f64>()),
        // chrono is answered in `dispatch_call`, Utc/Local/DateTime.
        // jsonwebtoken -----------------------------------------------------
        P::JsonwebtokenEncode => super::jwt_bridge::jwt_encode(args)?,
        // tempfile ---------------------------------------------------------
        P::TempfileTempdir => match tempfile::tempdir() {
            Ok(d) => Value::ok(Native::TempDir(d).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        P::TempfileTempfile => match tempfile::tempfile() {
            Ok(f) => Value::ok(Native::File(std::io::BufReader::new(f)).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        P::NamedTempFileNew => match tempfile::NamedTempFile::new() {
            Ok(f) => Value::ok(Native::NamedTempFile(f).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // winreg -----------------------------------------------------------
        P::RegKeyPredef => super::winreg_bridge::predef(args),
        // windows-service --------------------------------------------------
        P::ServiceManagerLocalComputer => super::service_bridge::local_computer(args),
        // wmi --------------------------------------------------------------
        P::WMIConnectionNew => super::wmi_bridge::connection(args, true),
        P::WMIConnectionWithNamespacePath => super::wmi_bridge::connection(args, false),
        // crossterm --------------------------------------------------------
        P::TerminalSize => terminal_size(),
        // terminal-light ---------------------------------------------------
        P::TerminalLightLuma => terminal_luma(),
        _ => return Ok(None),
    }))
}

/// `crossterm::terminal::size`. The pair is columns then rows, the order the
/// real call returns, which is the opposite of how a `Rect` is written.
fn terminal_size() -> Value {
    match crossterm::terminal::size() {
        Ok((cols, rows)) => Value::ok(Value::tuple(vec![
            Value::Int(i64::from(cols)),
            Value::Int(i64::from(rows)),
        ])),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// `terminal_light::luma`, the background brightness from 0 for black to 1 for
/// white. The crate asks the terminal over an escape sequence and falls back to
/// `$COLORFGBG`, so an error means neither source answered.
fn terminal_luma() -> Value {
    match terminal_light::luma() {
        Ok(luma) => Value::ok(Value::F32(luma)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Recognize a base64 engine constant name and build a marker value carrying
/// which alphabet it uses, so `.encode`/`.decode` can pick the right engine.
pub(super) fn base64_engine(id: P) -> Option<Value> {
    let kind = match id {
        P::Standard | P::Base64Standard => "standard",
        P::StandardNoPad | P::Base64StandardNoPad => "standard_no_pad",
        P::UrlSafe | P::Base64UrlSafe => "url_safe",
        P::UrlSafeNoPad | P::Base64UrlSafeNoPad => "url_safe_no_pad",
        _ => return None,
    };
    Some(Value::struct_of(
        "Base64Engine",
        [("kind".into(), Value::str(kind))],
    ))
}

pub(super) fn base64_method(s: &StructData, method: &MethodName, args: &[Value]) -> Result<Value> {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
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
    Ok(match method.id {
        B::Encode => Value::str(pick!(encode, value_to_bytes(args.first()))),
        B::Decode => {
            let input = args.first().map(Value::display).unwrap_or_default();
            match pick!(decode, &input) {
                Ok(b) => Value::ok(bytes_to_vec(&b)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => bail!("unknown method `{method}` on a base64 engine"),
    })
}

pub(super) fn rng_method(name: &MethodName, args: &[Value]) -> Result<Value> {
    use rand::RngExt;
    let mut rng = rand::rng();
    Ok(match name.id {
        B::RandomRange | B::GenRange => match args.first() {
            Some(Value::Range {
                start,
                end,
                inclusive,
            }) => {
                let hi = if *inclusive { end + 1 } else { *end };
                if hi > *start {
                    Value::Int(rng.random_range(*start..hi))
                } else {
                    Value::Int(*start)
                }
            }
            _ => bail!("random_range needs a range"),
        },
        B::RandomBool | B::GenBool => {
            let p = match args.first() {
                Some(Value::Float(f)) => *f,
                Some(Value::Int(i)) => AsPrimitive::<f64>::as_(*i),
                _ => 0.5,
            };
            Value::Bool(rng.random_bool(p.clamp(0.0, 1.0)))
        }
        B::Random | B::Gen => Value::Float(rng.random::<f64>()),
        B::FillBytes | B::Fill => {
            if let Some(Value::Vec(v)) = args.first() {
                let mut buf = v.lock();
                for slot in buf.iter_mut() {
                    *slot = Value::Int(i64::from(rng.random::<u8>()));
                }
            }
            Value::Unit
        }
        _ => bail!("unknown method `{name}` on Rng"),
    })
}

/// Methods on an in-progress `Sha256` hasher handle. `update` feeds bytes and
/// returns unit like the real `Digest::update`, `chain_update` feeds then hands
/// the same hasher back for chaining, and `finalize` reads the digest as a byte
/// vec. `finalize` clones the hasher rather than consuming it, so the byte vec
/// pairs with `hex::encode` exactly as the compiled crate does.
pub(super) fn sha256_method(
    handle: &Arc<Mutex<Native>>,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    use sha2::Digest;
    let mut h = handle.lock();
    let Native::Sha256(hasher) = &mut *h else {
        return Ok(None);
    };
    Ok(Some(match method.id {
        B::Update => {
            hasher.update(value_to_bytes(args.first()));
            Value::Unit
        }
        B::ChainUpdate => {
            hasher.update(value_to_bytes(args.first()));
            Value::Native(handle.clone())
        }
        B::Finalize => bytes_to_vec(&hasher.clone().finalize()),
        _ => return Ok(None),
    }))
}
