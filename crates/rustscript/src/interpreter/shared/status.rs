//! The http status, header value and exit status method cores.

use crate::interpreter::bytecode::BuiltinId;

pub(crate) enum StatusOut {
    Int(i64),
    Bool(bool),
}

pub(crate) fn status_core(name: BuiltinId, code: i64) -> Option<StatusOut> {
    Some(match name {
        BuiltinId::AsU16 | BuiltinId::AsInt => StatusOut::Int(code),
        BuiltinId::IsSuccess => StatusOut::Bool((200..300).contains(&code)),
        BuiltinId::IsClientError => StatusOut::Bool((400..500).contains(&code)),
        BuiltinId::IsServerError => StatusOut::Bool((500..600).contains(&code)),
        _ => return None,
    })
}

pub(crate) enum HeaderOut {
    /// `to_str` gives `Ok(text)` like the real fallible accessor
    Ok(String),
    Text(String),
}

pub(crate) fn header_value_core(name: BuiltinId, text: String) -> Option<HeaderOut> {
    Some(match name {
        BuiltinId::ToStr => HeaderOut::Ok(text),
        BuiltinId::AsStr | BuiltinId::AsString | BuiltinId::ToString => HeaderOut::Text(text),
        _ => return None,
    })
}

pub(crate) enum ExitOut {
    Bool(bool),
    /// `None` after death by signal
    OptInt(Option<i64>),
}

pub(crate) fn exit_status_core(
    name: BuiltinId,
    success: bool,
    code: Option<i64>,
) -> Option<ExitOut> {
    Some(match name {
        BuiltinId::Success => ExitOut::Bool(success),
        BuiltinId::Code => ExitOut::OptInt(code),
        _ => return None,
    })
}
