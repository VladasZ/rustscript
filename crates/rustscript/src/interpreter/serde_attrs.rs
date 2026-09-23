//! `#[serde(rename = "..")]`, `#[serde(rename_all = "..")]`,
//! `#[serde(skip_serializing_if = "Option::is_none")]` and `#[serde(default)]`.
//!
//! Each parser reads 1 key and steps over the value of every other key, so
//! `#[serde(default = "f", rename = "x")]` gives both.

/// True for `skip_serializing_if = "Option::is_none"`, the one predicate serialization honors.
pub(super) fn serde_skip_none(field: &syn::Field) -> bool {
    let mut skip = false;
    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        if attr
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("skip_serializing_if")
                    && let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    skip = lit.value() == "Option::is_none";
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })
            .is_err()
        {
            return false;
        }
    }
    skip
}

pub(super) fn serde_rename(field: &syn::Field) -> Option<String> {
    let mut renamed = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        if attr
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("rename")
                    && let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    renamed = Some(lit.value());
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })
            .is_err()
        {
            return None;
        }
    }
    renamed
}

#[derive(Clone, Copy)]
pub(super) enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

pub(super) fn serde_rename_all(attrs: &[syn::Attribute]) -> Option<RenameRule> {
    let mut rule = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        if attr
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all")
                    && let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    rule = RenameRule::parse(&lit.value());
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })
            .is_err()
        {
            return None;
        }
    }
    rule
}

impl RenameRule {
    fn parse(name: &str) -> Option<RenameRule> {
        Some(match name {
            "lowercase" => RenameRule::Lower,
            "UPPERCASE" => RenameRule::Upper,
            "PascalCase" => RenameRule::Pascal,
            "camelCase" => RenameRule::Camel,
            "snake_case" => RenameRule::Snake,
            "SCREAMING_SNAKE_CASE" => RenameRule::ScreamingSnake,
            "kebab-case" => RenameRule::Kebab,
            "SCREAMING-KEBAB-CASE" => RenameRule::ScreamingKebab,
            _ => return None,
        })
    }

    /// following serde's field rules
    pub(super) fn apply(self, field: &str) -> String {
        match self {
            RenameRule::Lower | RenameRule::Snake => field.to_string(),
            RenameRule::Upper | RenameRule::ScreamingSnake => field.to_ascii_uppercase(),
            RenameRule::Kebab => field.replace('_', "-"),
            RenameRule::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
            RenameRule::Pascal | RenameRule::Camel => {
                let mut out = String::with_capacity(field.len());
                let mut upper = matches!(self, RenameRule::Pascal);
                for ch in field.chars() {
                    if ch == '_' {
                        upper = true;
                    } else if upper {
                        out.extend(ch.to_uppercase());
                        upper = false;
                    } else {
                        out.push(ch);
                    }
                }
                out
            }
        }
    }
}

/// `#[serde(default)]` or `#[serde(default = "path")]` on a field.
pub(super) enum SerdeDefault {
    /// the field type's `Default`
    Type,
    /// a function that returns the value, called each time the field is missing
    Path(String),
}

pub(super) fn serde_default(field: &syn::Field) -> Option<SerdeDefault> {
    let mut found = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                found = Some(match meta.value() {
                    Ok(value) => SerdeDefault::Path(value.parse::<syn::LitStr>()?.value()),
                    Err(_) => SerdeDefault::Type,
                });
            } else {
                skip_value(&meta)?;
            }
            Ok(())
        });
        if parsed.is_err() {
            return None;
        }
    }
    found
}

/// Steps over `= value` or `(..)` of a key the caller does not read.
fn skip_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        meta.value()?.parse::<syn::Expr>()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_value(&inner))?;
    }
    Ok(())
}

/// The container attributes of an enum and the serde name of each variant.
pub(super) fn serde_enum(e: &syn::ItemEnum) -> super::enum_def::SerdeEnum {
    use super::enum_def::{SerdeEnum, SerdeRepr};
    let mut tag = None;
    let mut content = None;
    let mut untagged = false;
    for attr in &e.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("untagged") {
                untagged = true;
            } else if meta.path.is_ident("tag") {
                tag = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else if meta.path.is_ident("content") {
                content = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else {
                skip_value(&meta)?;
            }
            Ok(())
        });
    }
    let repr = match (untagged, tag, content) {
        (true, _, _) => SerdeRepr::Untagged,
        (false, Some(t), Some(c)) => SerdeRepr::Adjacent(t.into(), c.into()),
        (false, Some(t), None) => SerdeRepr::Internal(t.into()),
        (false, None, _) => SerdeRepr::External,
    };
    let rule = serde_rename_all(&e.attrs);
    let names = e
        .variants
        .iter()
        .map(|v| {
            let own = v.ident.to_string();
            serde_rename_attrs(&v.attrs)
                .or_else(|| rule.map(|r| r.apply_variant(&own)))
                .unwrap_or(own)
                .into()
        })
        .collect();
    SerdeEnum { repr, names }
}

/// `rename` on anything with attributes, a variant here.
fn serde_rename_attrs(attrs: &[syn::Attribute]) -> Option<String> {
    let mut renamed = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename")
                && let Ok(value) = meta.value()
            {
                renamed = Some(value.parse::<syn::LitStr>()?.value());
            } else {
                skip_value(&meta)?;
            }
            Ok(())
        });
    }
    renamed
}

impl RenameRule {
    /// serde's rules for a `PascalCase` variant name
    pub(super) fn apply_variant(self, variant: &str) -> String {
        let snake = || {
            let mut out = String::with_capacity(variant.len() + 4);
            for (i, ch) in variant.char_indices() {
                if i > 0 && ch.is_uppercase() {
                    out.push('_');
                }
                out.extend(ch.to_lowercase());
            }
            out
        };
        match self {
            RenameRule::Pascal => variant.to_string(),
            RenameRule::Lower => variant.to_ascii_lowercase(),
            RenameRule::Upper => variant.to_ascii_uppercase(),
            RenameRule::Camel => {
                let mut chars = variant.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_lowercase().chain(chars).collect()
                })
            }
            RenameRule::Snake => snake(),
            RenameRule::ScreamingSnake => snake().to_ascii_uppercase(),
            RenameRule::Kebab => snake().replace('_', "-"),
            RenameRule::ScreamingKebab => snake().to_ascii_uppercase().replace('_', "-"),
        }
    }
}
