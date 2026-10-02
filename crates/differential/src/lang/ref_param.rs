//! How a closure that takes its item by reference names it.

use serde::{Deserialize, Serialize};

/// How a closure that is handed `&Item` names the item.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RefParam {
    /// `|r| { let x = r.clone(); body }`, the body owns a clone
    #[default]
    Cloned,
    /// `|&x| body`, a copy item taken out of the reference in the parameter
    Pat,
    /// `|x| body`, the body reads the item through `(*x)`
    Deref,
}

impl RefParam {
    /// The name the body reads the item by.
    pub fn local(self, bind: &str) -> String {
        match self {
            Self::Deref => format!("(*{bind})"),
            Self::Cloned | Self::Pat => bind.to_string(),
        }
    }

    pub fn feature(self) -> Option<&'static str> {
        match self {
            Self::Cloned => None,
            Self::Pat => Some("lang-closure-ref-pat"),
            Self::Deref => Some("lang-closure-ref-deref"),
        }
    }

    /// The whole closure. `ty` is the item type, stated when given.
    pub fn closure(self, pattern: &str, ty: Option<&str>, body: &str) -> String {
        match (self, ty) {
            (Self::Cloned, Some(ty)) => {
                format!("|diff_ref| {{ let {pattern}: {ty} = diff_ref.clone(); {body} }}")
            }
            (Self::Cloned, None) => {
                format!("|diff_ref| {{ let {pattern} = diff_ref.clone(); {body} }}")
            }
            (Self::Pat, Some(ty)) => format!("|&{pattern}: &{ty}| {body}"),
            (Self::Pat, None) => format!("|&{pattern}| {body}"),
            (Self::Deref, Some(ty)) => format!("|{pattern}: &{ty}| {body}"),
            (Self::Deref, None) => format!("|{pattern}| {body}"),
        }
    }
}
