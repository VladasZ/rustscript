//! `anyhow::Error` as a chain of messages, the outermost context first. `{}` shows the first,
//! `{:#}` joins them with `: `, and `{:?}` lists the causes under `Caused by:` like anyhow.

use super::native::Native;
use super::value::Value;

pub(super) fn anyhow_value(chain: Vec<String>) -> Value {
    Native::Anyhow(chain).wrap()
}

/// The layers of any error, an anyhow error keeps its own and anything else is 1 layer.
pub(super) fn chain_of(error: &Value) -> Vec<String> {
    if let Value::Native(n) = error
        && let Native::Anyhow(chain) = &*n.lock()
    {
        return chain.clone();
    }
    vec![error.display()]
}

/// `.context(c)` on an error, a new outer layer over the old ones.
pub(super) fn with_context(context: String, cause: &Value) -> Value {
    let mut chain = vec![context];
    chain.extend(chain_of(cause));
    anyhow_value(chain)
}

/// `?` into an `anyhow::Result` wraps the error, one already wrapped stays as it is.
pub(super) fn into_anyhow(error: Value) -> Value {
    if let Value::Native(n) = &error
        && matches!(&*n.lock(), Native::Anyhow(_))
    {
        return error;
    }
    anyhow_value(vec![error.display()])
}

/// `{}` of an anyhow error is its outermost message, `{:#}` joins the whole chain. `None`
/// for any other value.
pub(super) fn display(value: &Value, alternate: bool) -> Option<String> {
    let chain = chain_if_anyhow(value)?;
    Some(if alternate {
        chain.join(": ")
    } else {
        chain.first().cloned().unwrap_or_default()
    })
}

pub(super) fn debug(chain: &[String]) -> String {
    let mut out = chain.first().cloned().unwrap_or_default();
    match &chain[1.min(chain.len())..] {
        [] => {}
        [one] => {
            out.push_str("\n\nCaused by:\n    ");
            out.push_str(one);
        }
        causes => {
            out.push_str("\n\nCaused by:");
            for (i, cause) in causes.iter().enumerate() {
                out.push_str(&format!("\n    {i}: {cause}"));
            }
        }
    }
    out
}

/// The chain of a value that is an anyhow error.
pub(super) fn chain_if_anyhow(value: &Value) -> Option<Vec<String>> {
    let Value::Native(n) = value else {
        return None;
    };
    match &*n.lock() {
        Native::Anyhow(chain) => Some(chain.clone()),
        _ => None,
    }
}
