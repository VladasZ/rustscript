//! `#[serde(rename = "..")]`, `#[serde(rename_all = "..")]` and
//! `#[serde(skip_serializing_if = "Option::is_none")]`.

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
