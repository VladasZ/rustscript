//! Format specs. Every observation prints through one.

use serde::{Deserialize, Serialize};

use crate::lang::ty::Ty;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FmtTrait {
    Display,
    Debug,
    LowerHex,
    UpperHex,
    Octal,
    Binary,
    LowerExp,
    UpperExp,
}

impl FmtTrait {
    fn token(self) -> &'static str {
        match self {
            Self::Display => "",
            Self::Debug => "?",
            Self::LowerHex => "x",
            Self::UpperHex => "X",
            Self::Octal => "o",
            Self::Binary => "b",
            Self::LowerExp => "e",
            Self::UpperExp => "E",
        }
    }

    pub fn applies_to(self, ty: &Ty) -> bool {
        match self {
            Self::Display => ty.has_display(),
            Self::Debug => true,
            Self::LowerHex | Self::UpperHex | Self::Octal | Self::Binary => ty.is_int(),
            Self::LowerExp | Self::UpperExp => ty.is_numeric(),
        }
    }

    pub fn feature(self) -> &'static str {
        match self {
            Self::Display => "lang-fmt-display",
            Self::Debug => "lang-fmt-debug",
            Self::LowerHex | Self::UpperHex => "lang-fmt-hex",
            Self::Octal => "lang-fmt-octal",
            Self::Binary => "lang-fmt-binary",
            Self::LowerExp | Self::UpperExp => "lang-fmt-exp",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Right,
    Center,
}

impl Align {
    fn token(self) -> char {
        match self {
            Self::Left => '<',
            Self::Right => '>',
            Self::Center => '^',
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FmtSpec {
    pub fmt: FmtTrait,
    pub alternate: bool,
    pub width: Option<u8>,
    pub fill: Option<char>,
    pub align: Option<Align>,
    pub plus: bool,
    pub zero: bool,
    pub precision: Option<u8>,
}

impl FmtSpec {
    pub const DEBUG: FmtSpec = FmtSpec {
        fmt: FmtTrait::Debug,
        alternate: false,
        width: None,
        fill: None,
        align: None,
        plus: false,
        zero: false,
        precision: None,
    };

    pub const DISPLAY: FmtSpec = FmtSpec {
        fmt: FmtTrait::Display,
        ..Self::DEBUG
    };

    pub fn plain(fmt: FmtTrait) -> Self {
        Self { fmt, ..Self::DEBUG }
    }

    /// The text after the colon. Empty for a bare `{}`.
    pub fn body(&self) -> String {
        let mut out = String::new();
        if let Some(fill) = self.fill {
            out.push(fill);
            out.push(self.align.unwrap_or(Align::Right).token());
        } else if let Some(align) = self.align {
            out.push(align.token());
        }
        if self.plus {
            out.push('+');
        }
        if self.alternate {
            out.push('#');
        }
        if self.zero {
            out.push('0');
        }
        if let Some(width) = self.width {
            out.push_str(&width.to_string());
        }
        if let Some(precision) = self.precision {
            out.push('.');
            out.push_str(&precision.to_string());
        }
        out.push_str(self.fmt.token());
        out
    }

    /// `{}` or `{:spec}`
    pub fn placeholder(&self) -> String {
        self.placeholder_for("")
    }

    /// `{arg}` or `{arg:spec}`, `arg` a position or a name
    pub fn placeholder_for(&self, arg: &str) -> String {
        let body = self.body();
        if body.is_empty() {
            format!("{{{arg}}}")
        } else {
            format!("{{{arg}:{body}}}")
        }
    }

    pub fn applies_to(&self, ty: &Ty) -> bool {
        if !self.fmt.applies_to(ty) {
            return false;
        }
        if self.precision.is_some() {
            let ok = match self.fmt {
                // precision on an integer is legal but observes nothing
                FmtTrait::Display => matches!(ty, Ty::Float(_) | Ty::Str),
                FmtTrait::Debug | FmtTrait::LowerExp | FmtTrait::UpperExp => {
                    matches!(ty, Ty::Float(_))
                }
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        if (self.plus || self.zero) && !ty.is_numeric() {
            return false;
        }
        true
    }

    pub fn features(&self, out: &mut std::collections::BTreeSet<&'static str>) {
        out.insert(self.fmt.feature());
        if self.alternate {
            out.insert("lang-fmt-alternate");
        }
        if self.width.is_some() {
            out.insert("lang-fmt-width");
        }
        if self.fill.is_some() || self.align.is_some() {
            out.insert("lang-fmt-align");
        }
        if self.plus {
            out.insert("lang-fmt-plus");
        }
        if self.zero {
            out.insert("lang-fmt-zero");
        }
        if self.precision.is_some() {
            out.insert("lang-fmt-precision");
        }
    }
}
