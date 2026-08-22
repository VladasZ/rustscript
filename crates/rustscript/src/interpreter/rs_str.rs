//! The string payload behind `Value::Str`. `push` appends in place when this handle is the only one,
//! so an append loop is linear. A shared buffer is copied once on the first append.

use std::fmt::{self, Debug, Display};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

#[derive(Clone)]
pub struct RsStr(Arc<String>);

impl RsStr {
    pub fn push(&mut self, c: char) {
        Arc::make_mut(&mut self.0).push(c);
    }

    pub fn push_str(&mut self, s: &str) {
        Arc::make_mut(&mut self.0).push_str(s);
    }
}

impl Deref for RsStr {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RsStr {
    fn as_ref(&self) -> &str {
        self
    }
}

impl From<&str> for RsStr {
    fn from(s: &str) -> Self {
        Self(Arc::new(s.to_owned()))
    }
}

impl From<String> for RsStr {
    fn from(s: String) -> Self {
        Self(Arc::new(s))
    }
}

impl From<&String> for RsStr {
    fn from(s: &String) -> Self {
        Self(Arc::new(s.clone()))
    }
}

impl From<std::borrow::Cow<'_, str>> for RsStr {
    fn from(s: std::borrow::Cow<'_, str>) -> Self {
        Self(Arc::new(s.into_owned()))
    }
}

impl PartialEq for RsStr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Eq for RsStr {}

/// Hashes as the bare `str` so a key hashes the same however it was built.
impl Hash for RsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl Display for RsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Debug for RsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}
