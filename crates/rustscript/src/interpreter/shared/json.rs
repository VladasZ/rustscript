//! The json shape tests and RFC 6901 pointers.

use crate::interpreter::bytecode::BuiltinId;

/// Parsed json is held as plain values, so the serde type tests are shape tests.
#[derive(Clone, Copy)]
pub(crate) enum JsonKind {
    Object,
    Array,
    Str,
    Bool,
    /// real value, so the range tests work
    Int(i128),
    Float,
    Null,
    Other,
}

/// Runs before the per type dispatch, which returns early for the hot receivers.
pub(crate) fn json_type_test(kind: JsonKind, name: BuiltinId) -> Option<bool> {
    Some(match name {
        BuiltinId::IsObject => matches!(kind, JsonKind::Object),
        BuiltinId::IsArray => matches!(kind, JsonKind::Array),
        BuiltinId::IsString => matches!(kind, JsonKind::Str),
        BuiltinId::IsBoolean => matches!(kind, JsonKind::Bool),
        BuiltinId::IsNumber => matches!(kind, JsonKind::Int(_) | JsonKind::Float),
        // serde checks by range, a negative number is not a u64
        BuiltinId::IsI64 => matches!(kind, JsonKind::Int(v) if i64::try_from(v).is_ok()),
        BuiltinId::IsU64 => matches!(kind, JsonKind::Int(v) if u64::try_from(v).is_ok()),
        BuiltinId::IsF64 => matches!(kind, JsonKind::Float),
        BuiltinId::IsNull => matches!(kind, JsonKind::Null),
        _ => return None,
    })
}

/// By name only, the caller decides if the receiver matches. A wrong shape gives None.
pub(crate) fn json_accessor(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::AsStr
            | BuiltinId::AsI64
            | BuiltinId::AsU64
            | BuiltinId::AsF64
            | BuiltinId::AsBool
            | BuiltinId::AsArray
            | BuiltinId::AsArrayMut
            | BuiltinId::AsObject
            | BuiltinId::AsObjectMut
    )
}

/// RFC 6901. An empty pointer is the whole value. `~1` and `~0` escape slash and tilde.
pub(crate) fn json_pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    Some(
        pointer
            .split('/')
            .skip(1)
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// serde rejects a leading plus and a leading zero
pub(crate) fn json_pointer_index(token: &str) -> Option<usize> {
    if token.starts_with('+') || (token.starts_with('0') && token.len() != 1) {
        return None;
    }
    token.parse().ok()
}
