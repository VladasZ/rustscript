//! Patterns for `match` arms.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::ty::{IntWidth, Ty};
use crate::lang::user::UserShape;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Pat {
    Wild,
    Bind {
        name: String,
        ty: Ty,
    },
    IntLit {
        width: IntWidth,
        value: i128,
    },
    /// `lo..=hi` or `lo..hi`.
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
    /// `Name { field: pat, .. }`, only the listed fields.
    Struct {
        shape: Box<UserShape>,
        fields: Vec<(usize, Pat)>,
    },
    /// Element binds are references, the arm prologue clones them.
    Slice {
        elem: Ty,
        prefix: Vec<Pat>,
        /// `None` for no rest, `Some(None)` for a bare `..`, `Some(Some(n))`
        /// for `n @ ..`.
        rest: Option<Option<String>>,
        suffix: Vec<Pat>,
    },
}

impl Pat {
    pub fn render(&self) -> String {
        match self {
            Self::Wild => "_".to_string(),
            Self::Bind { name, .. } => name.clone(),
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

    pub fn bindings(&self, out: &mut Vec<(String, Ty)>) {
        match self {
            Self::Bind { name, ty } => out.push((name.clone(), ty.clone())),
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

    /// Whether no `_` arm is needed after it.
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Self::Wild | Self::Bind { .. } => true,
            Self::Tuple(items) => items.iter().all(Pat::is_irrefutable),
            _ => false,
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        match self {
            Self::Wild | Self::Bind { .. } => {}
            Self::IntLit { .. } | Self::BoolLit(_) | Self::CharLit(_) => {
                out.insert("lang-pat-literal");
            }
            Self::IntRange { .. } => {
                out.insert("lang-pat-range");
            }
            Self::Some(inner) | Self::Ok(inner) | Self::Err(inner) => {
                out.insert(if matches!(self, Self::Some(_)) {
                    "lang-pat-option"
                } else {
                    "lang-pat-result"
                });
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
