//! Patterns for `match` arms.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
use crate::lang::ty::{IntWidth, Ty};
use crate::lang::user::UserShape;

/// How a binding takes its value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum BindBy {
    #[default]
    Value,
    /// `ref name`, the arm sees the value through `(*name)` and can only clone it
    Ref,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Pat {
    Wild,
    Bind {
        name: String,
        ty: Ty,
        #[serde(default)]
        by: BindBy,
    },
    /// `name @ pat`, the inner pattern binds nothing
    At {
        name: String,
        ty: Ty,
        pat: Box<Pat>,
    },
    /// `a | b | c`, no alternative binds
    Or(Vec<Pat>),
    /// a `const` item named as a pattern
    Const {
        name: String,
    },
    /// `"text"`, against a `&str` scrutinee
    StrLit(String),
    /// `'a'..='z'`
    CharRange {
        lo: char,
        hi: char,
    },
    IntLit {
        width: IntWidth,
        value: i128,
    },
    /// `lo..=hi` or `lo..hi`
    IntRange {
        width: IntWidth,
        lo: i128,
        hi: i128,
        inclusive: bool,
    },
    BoolLit(bool),
    CharLit(char),
    Some(Box<Pat>),
    None,
    Ok(Box<Pat>),
    Err(Box<Pat>),
    Tuple(Vec<Pat>),
    Variant {
        shape: Box<UserShape>,
        variant: usize,
        payload: Vec<Pat>,
    },
    /// `Name { field: pat, .. }`, only the listed fields
    Struct {
        shape: Box<UserShape>,
        fields: Vec<(usize, Pat)>,
    },
    /// element binds are references, the arm prologue clones them
    Slice {
        elem: Ty,
        prefix: Vec<Pat>,
        /// `None` for no rest, `Some(None)` for a bare `..`, `Some(Some(n))` for `n @ ..`
        rest: Option<Option<String>>,
        suffix: Vec<Pat>,
    },
}

impl Pat {
    pub fn render(&self) -> String {
        match self {
            Self::Wild => "_".to_string(),
            Self::Bind {
                name,
                by: BindBy::Value,
                ..
            }
            | Self::Const { name } => name.clone(),
            Self::Bind {
                name,
                by: BindBy::Ref,
                ..
            } => format!("ref {name}"),
            Self::At { name, pat, .. } => format!("{name} @ {}", pat.render_grouped()),
            Self::Or(items) => {
                let rendered: Vec<String> = items.iter().map(Pat::render).collect();
                rendered.join(" | ")
            }
            Self::StrLit(value) => format!("{value:?}"),
            Self::CharRange { lo, hi } => format!("{lo:?}..={hi:?}"),
            Self::IntLit { width, value } => render_int(*width, *value),
            Self::IntRange {
                width,
                lo,
                hi,
                inclusive,
            } => format!(
                "{}{}{}",
                render_int(*width, *lo),
                if *inclusive { "..=" } else { ".." },
                render_int(*width, *hi)
            ),
            Self::BoolLit(value) => value.to_string(),
            Self::CharLit(value) => format!("{value:?}"),
            Self::Some(inner) => format!("Some({})", inner.render()),
            Self::None => "None".to_string(),
            Self::Ok(inner) => format!("Ok({})", inner.render()),
            Self::Err(inner) => format!("Err({})", inner.render()),
            Self::Tuple(items) => {
                let rendered: Vec<String> = items.iter().map(Pat::render).collect();
                match items.len() {
                    1 => format!("({},)", rendered[0]),
                    _ => format!("({})", rendered.join(", ")),
                }
            }
            Self::Variant {
                shape,
                variant,
                payload,
            } => {
                let name = &shape.variants()[*variant].name;
                if payload.is_empty() {
                    format!("{}::{name}", shape.name)
                } else {
                    let rendered: Vec<String> = payload.iter().map(Pat::render).collect();
                    format!("{}::{name}({})", shape.name, rendered.join(", "))
                }
            }
            Self::Struct { shape, fields } => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .map(|(index, pat)| {
                        format!("{}: {}", shape.fields()[*index].name, pat.render())
                    })
                    .collect();
                parts.push("..".to_string());
                format!("{} {{ {} }}", shape.name, parts.join(", "))
            }
            Self::Slice {
                prefix,
                rest,
                suffix,
                ..
            } => {
                let mut parts: Vec<String> = prefix.iter().map(Pat::render).collect();
                match rest {
                    Some(Some(name)) => parts.push(format!("{name} @ ..")),
                    Some(None) => parts.push("..".to_string()),
                    None => {}
                }
                parts.extend(suffix.iter().map(Pat::render));
                format!("[{}]", parts.join(", "))
            }
        }
    }

    /// An or pattern under `@` or at the top of a `let` needs its own parentheses.
    pub fn render_grouped(&self) -> String {
        match self {
            Self::Or(_) => format!("({})", self.render()),
            other => other.render(),
        }
    }

    /// The names an arm body reads, with their types. A `ref` binding is read through a
    /// deref, so its name is `(*name)`.
    pub fn bindings(&self, out: &mut Vec<(String, Ty)>) {
        match self {
            Self::Bind {
                name,
                ty,
                by: BindBy::Value,
            }
            | Self::At { name, ty, .. } => out.push((name.clone(), ty.clone())),
            Self::Bind {
                name,
                ty,
                by: BindBy::Ref,
            } => out.push((format!("(*{name})"), ty.clone())),
            Self::Some(inner) | Self::Ok(inner) | Self::Err(inner) => inner.bindings(out),
            Self::Tuple(items) => {
                for item in items {
                    item.bindings(out);
                }
            }
            Self::Variant { payload, .. } => {
                for pat in payload {
                    pat.bindings(out);
                }
            }
            Self::Struct { fields, .. } => {
                for (_, pat) in fields {
                    pat.bindings(out);
                }
            }
            Self::Slice {
                elem,
                prefix,
                rest,
                suffix,
            } => {
                for pat in prefix.iter().chain(suffix) {
                    pat.bindings(out);
                }
                if let Some(Some(name)) = rest {
                    out.push((name.clone(), Ty::vec_of(elem.clone())));
                }
            }
            _ => {}
        }
    }

    /// The binding a `ref` binding of this pattern borrows through the scrutinee, held until the
    /// bindings go. None when nothing binds by `ref` or the scrutinee is a temporary.
    pub fn pins<'e>(&self, scrutinee: &'e Expr) -> Option<&'e str> {
        if self.borrowed().is_empty() {
            return None;
        }
        scrutinee.place_root()
    }

    /// The bindings that stand behind a reference, so the body may only clone them.
    pub fn borrowed(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.walk(&mut |pat| {
            if let Self::Bind {
                name,
                by: BindBy::Ref,
                ..
            } = pat
            {
                out.insert(format!("(*{name})"));
            }
        });
        out
    }

    /// Every pattern in the tree, this one first.
    pub fn walk(&self, visit: &mut impl FnMut(&Pat)) {
        visit(self);
        match self {
            Self::Some(inner)
            | Self::Ok(inner)
            | Self::Err(inner)
            | Self::At { pat: inner, .. } => {
                inner.walk(visit);
            }
            Self::Tuple(items) | Self::Or(items) => {
                for item in items {
                    item.walk(visit);
                }
            }
            Self::Variant { payload, .. } => {
                for pat in payload {
                    pat.walk(visit);
                }
            }
            Self::Struct { fields, .. } => {
                for (_, pat) in fields {
                    pat.walk(visit);
                }
            }
            Self::Slice { prefix, suffix, .. } => {
                for pat in prefix.iter().chain(suffix) {
                    pat.walk(visit);
                }
            }
            _ => {}
        }
    }

    /// Whether the pattern names an item of its own program, a user type or a const. Such a
    /// pattern can't travel to another program.
    pub fn names_item(&self) -> bool {
        let mut found = false;
        self.walk(&mut |pat| {
            if matches!(
                pat,
                Self::Variant { .. } | Self::Struct { .. } | Self::Const { .. }
            ) {
                found = true;
            }
        });
        found
    }

    /// A variant pattern whose payload can't miss, so the variant itself is covered.
    pub fn is_irrefutable_payload(&self) -> bool {
        match self {
            Self::Variant { payload, .. } => payload.iter().all(Pat::is_irrefutable),
            _ => false,
        }
    }

    /// Whether no `_` arm is needed after it.
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Self::Wild | Self::Bind { .. } => true,
            Self::At { pat, .. } => pat.is_irrefutable(),
            Self::Tuple(items) => items.iter().all(Pat::is_irrefutable),
            Self::Struct { fields, .. } => fields.iter().all(|(_, pat)| pat.is_irrefutable()),
            _ => false,
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        match self {
            Self::Wild
            | Self::Bind {
                by: BindBy::Value, ..
            } => {}
            Self::Bind {
                by: BindBy::Ref, ..
            } => {
                out.insert("lang-pat-ref");
            }
            Self::At { pat, .. } => {
                out.insert("lang-pat-at");
                pat.features(out);
            }
            Self::Or(items) => {
                out.insert("lang-pat-or");
                for item in items {
                    item.features(out);
                }
            }
            Self::Const { .. } => {
                out.insert("lang-pat-const");
            }
            Self::StrLit(_) => {
                out.insert("lang-pat-str");
            }
            Self::IntLit { .. } | Self::BoolLit(_) | Self::CharLit(_) => {
                out.insert("lang-pat-literal");
            }
            Self::IntRange { .. } | Self::CharRange { .. } => {
                out.insert("lang-pat-range");
            }
            Self::Some(inner) | Self::Ok(inner) | Self::Err(inner) => {
                out.insert(if matches!(self, Self::Some(_)) {
                    "lang-pat-option"
                } else {
                    "lang-pat-result"
                });
                if !matches!(**inner, Self::Wild | Self::Bind { .. }) {
                    out.insert("lang-pat-nested");
                }
                inner.features(out);
            }
            Self::None => {
                out.insert("lang-pat-option");
            }
            Self::Tuple(items) => {
                out.insert("lang-pat-tuple");
                for item in items {
                    item.features(out);
                }
            }
            Self::Variant { payload, .. } => {
                out.insert("lang-pat-enum");
                for pat in payload {
                    pat.features(out);
                }
            }
            Self::Struct { fields, .. } => {
                out.insert("lang-pat-struct");
                for (_, pat) in fields {
                    pat.features(out);
                }
            }
            Self::Slice {
                prefix,
                rest,
                suffix,
                ..
            } => {
                out.insert("lang-pat-slice");
                if rest.is_some() {
                    out.insert("lang-pat-slice-rest");
                }
                for pat in prefix.iter().chain(suffix) {
                    pat.features(out);
                }
            }
        }
    }
}

fn render_int(width: IntWidth, value: i128) -> String {
    format!("{value}{}", width.rust())
}
