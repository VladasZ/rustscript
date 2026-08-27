//! Bridge for `ed25519_dalek`, the key and signature types release scripts sign artifacts with.

use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName, PathId};
use super::crates_bridge::bytes_to_vec;
use super::native::Native;
use super::native_methods::value_to_bytes;
use super::value::Value;

/// None when the id is not an ed25519 path.
pub(super) fn ed25519_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::SigningKeyFromBytes => {
            Native::SigningKey(ed25519_dalek::SigningKey::from_bytes(&key_bytes(args)?)).wrap()
        }
        PathId::SigningKeyTryFrom => {
            match ed25519_dalek::SigningKey::try_from(value_to_bytes(args.first()).as_slice()) {
                Ok(k) => Value::ok(Native::SigningKey(k).wrap()),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        PathId::VerifyingKeyFromBytes => {
            match ed25519_dalek::VerifyingKey::from_bytes(&key_bytes(args)?) {
                Ok(k) => Value::ok(Native::VerifyingKey(k).wrap()),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        PathId::VerifyingKeyTryFrom => {
            match ed25519_dalek::VerifyingKey::try_from(value_to_bytes(args.first()).as_slice()) {
                Ok(k) => Value::ok(Native::VerifyingKey(k).wrap()),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        PathId::SignatureFromBytes => {
            let bytes = value_to_bytes(args.first());
            let Ok(arr) = <[u8; 64]>::try_from(bytes.as_slice()) else {
                bail!("Signature::from_bytes needs 64 bytes, got {}", bytes.len());
            };
            Native::Signature(ed25519_dalek::Signature::from_bytes(&arr)).wrap()
        }
        PathId::SignatureFromSlice => {
            match ed25519_dalek::Signature::from_slice(&value_to_bytes(args.first())) {
                Ok(s) => Value::ok(Native::Signature(s).wrap()),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => return Ok(None),
    }))
}

/// The 32 byte key both key types are built from.
fn key_bytes(args: &[Value]) -> Result<[u8; 32]> {
    let bytes = value_to_bytes(args.first());
    match <[u8; 32]>::try_from(bytes.as_slice()) {
        Ok(arr) => Ok(arr),
        Err(_) => bail!("an ed25519 key needs 32 bytes, got {}", bytes.len()),
    }
}

fn signature_arg(args: &[Value]) -> Result<ed25519_dalek::Signature> {
    match args.get(1) {
        Some(Value::Native(h)) => match &*h.lock() {
            Native::Signature(s) => Ok(*s),
            _ => bail!("expected a Signature"),
        },
        _ => bail!("expected a Signature"),
    }
}

pub(super) fn signing_key_method(
    handle: &Arc<Mutex<Native>>,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    use ed25519_dalek::Signer;
    let h = handle.lock();
    let Native::SigningKey(key) = &*h else {
        return Ok(None);
    };
    Ok(Some(match method.id {
        BuiltinId::Sign => Native::Signature(key.sign(&value_to_bytes(args.first()))).wrap(),
        BuiltinId::VerifyingKey => Native::VerifyingKey(key.verifying_key()).wrap(),
        BuiltinId::ToBytes => bytes_to_vec(&key.to_bytes()),
        BuiltinId::AsBytes => bytes_to_vec(key.as_bytes()),
        _ => return Ok(None),
    }))
}

pub(super) fn verifying_key_method(
    handle: &Arc<Mutex<Native>>,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    use ed25519_dalek::Verifier;
    let h = handle.lock();
    let Native::VerifyingKey(key) = &*h else {
        return Ok(None);
    };
    let verified = |ok: std::result::Result<(), ed25519_dalek::SignatureError>| match ok {
        Ok(()) => Value::ok(Value::Unit),
        Err(e) => Value::err(Value::str(e.to_string())),
    };
    Ok(Some(match method.id {
        BuiltinId::ToBytes => bytes_to_vec(&key.to_bytes()),
        BuiltinId::AsBytes => bytes_to_vec(key.as_bytes()),
        BuiltinId::Verify => {
            verified(key.verify(&value_to_bytes(args.first()), &signature_arg(args)?))
        }
        BuiltinId::VerifyStrict => {
            verified(key.verify_strict(&value_to_bytes(args.first()), &signature_arg(args)?))
        }
        _ => return Ok(None),
    }))
}

pub(super) fn signature_method(
    handle: &Arc<Mutex<Native>>,
    method: &MethodName,
) -> Result<Option<Value>> {
    let h = handle.lock();
    let Native::Signature(sig) = &*h else {
        return Ok(None);
    };
    Ok(Some(match method.id {
        BuiltinId::ToBytes => bytes_to_vec(&sig.to_bytes()),
        BuiltinId::ToVec => bytes_to_vec(&sig.to_vec()),
        _ => return Ok(None),
    }))
}
