//! Where the items of a block live. A block with a layout puts its types, consts and fns into a
//! `mod`, and `main` reaches them through a path or a `use`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Access {
    /// `diff_mod0::name`
    Path,
    /// `crate::diff_mod0::name`
    CratePath,
    /// `use diff_mod0 as diff_al0;` above `main`, then `diff_al0::name`
    Alias,
    /// `use diff_mod0::{a, b};` above `main`
    UseItems,
    /// `use diff_mod0::*;` above `main`
    UseGlob,
    /// `use diff_mod0::{a, b};` inside `main`
    LocalUse,
}

impl Access {
    /// Whether `main` names every item through a path. Only then may 2 modules share a name.
    pub fn by_path(self) -> bool {
        matches!(self, Self::Path | Self::CratePath | Self::Alias)
    }

    fn feature(self) -> &'static str {
        match self {
            Self::Path => "lang-mod-path",
            Self::CratePath => "lang-mod-crate-path",
            Self::Alias => "lang-mod-alias",
            Self::UseItems => "lang-mod-use",
            Self::UseGlob => "lang-mod-glob",
            Self::LocalUse => "lang-mod-local-use",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModLayout {
    /// the block tag, so 2 blocks get 2 modules
    pub tag: usize,
    /// `mod diff_mod0 { pub mod diff_inner { .. } }`, the inner name is the same in every block
    pub nested: bool,
    pub access: Access,
    /// The fns are named by position, so 2 blocks declare the same fn name in 2 modules.
    pub shared_fns: bool,
}

impl ModLayout {
    fn path(&self) -> String {
        if self.nested {
            format!("diff_mod{}::diff_inner", self.tag)
        } else {
            format!("diff_mod{}", self.tag)
        }
    }

    fn alias(&self) -> String {
        format!("diff_al{}", self.tag)
    }

    /// What `main` writes in front of an item name.
    fn prefix(&self) -> Option<String> {
        match self.access {
            Access::Path => Some(self.path()),
            Access::CratePath => Some(format!("crate::{}", self.path())),
            Access::Alias => Some(self.alias()),
            Access::UseItems | Access::UseGlob | Access::LocalUse => None,
        }
    }

    /// The fn names as the module declares them.
    fn renames(&self, fns: &[String]) -> BTreeMap<String, String> {
        if !self.shared_fns {
            return BTreeMap::new();
        }
        fns.iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), format!("diff_shared_{index}")))
            .collect()
    }

    /// The items inside their module. A glob of the parent brings in the trace type, the helpers
    /// and the std imports of the file.
    pub fn wrap_items(&self, items: &str, fns: &[String]) -> String {
        let items = rename_idents(items, &self.renames(fns));
        if self.nested {
            format!(
                "mod diff_mod{} {{\n    pub mod diff_inner {{\n        use super::super::*;\n\n{}    }}\n}}\n\n",
                self.tag,
                indent(&items, 2)
            )
        } else {
            format!(
                "mod diff_mod{} {{\n    use super::*;\n\n{}}}\n\n",
                self.tag,
                indent(&items, 1)
            )
        }
    }

    /// The `main` statements of the block with every item name spelled the way the access needs.
    pub fn qualify_body(&self, body: &str, names: &[String], fns: &[String]) -> String {
        let renames = self.renames(fns);
        let prefix = self.prefix();
        let map: BTreeMap<String, String> = names
            .iter()
            .map(|name| {
                let declared = renames.get(name).unwrap_or(name);
                let spelled = match &prefix {
                    Some(prefix) => format!("{prefix}::{declared}"),
                    None => declared.clone(),
                };
                (name.clone(), spelled)
            })
            .collect();
        let body = rename_idents(body, &map);
        if self.access == Access::LocalUse {
            format!("    use {}::{{{}}};\n{body}", self.path(), names.join(", "))
        } else {
            body
        }
    }

    /// The `use` lines above `main`.
    pub fn root_uses(&self, names: &[String]) -> String {
        match self.access {
            Access::Alias => format!("use {} as {};\n\n", self.path(), self.alias()),
            Access::UseItems => format!("use {}::{{{}}};\n\n", self.path(), names.join(", ")),
            Access::UseGlob => format!("use {}::*;\n\n", self.path()),
            Access::Path | Access::CratePath | Access::LocalUse => String::new(),
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert("lang-mod");
        out.insert(self.access.feature());
        if self.nested {
            out.insert("lang-mod-nested");
        }
        if self.shared_fns {
            out.insert("lang-mod-shared-fn");
        }
    }
}

fn indent(text: &str, levels: usize) -> String {
    let pad = "    ".repeat(levels);
    let mut out = String::new();
    for line in text.lines() {
        if !line.is_empty() {
            out.push_str(&pad);
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Replaces whole identifiers. Every item name carries the block tag, so a name is never a part of
/// another token.
fn rename_idents(text: &str, map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return text.to_string();
    }
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len());
        if end == 0 {
            let width = rest.chars().next().map_or(1, char::len_utf8);
            out.push_str(&rest[..width]);
            rest = &rest[width..];
            continue;
        }
        let word = &rest[..end];
        out.push_str(map.get(word).map_or(word, String::as_str));
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_takes_whole_identifiers_only() {
        let map = BTreeMap::from([("diff_fn_0_1".to_string(), "m::f".to_string())]);
        assert_eq!(
            rename_idents("diff_fn_0_1(diff_fn_0_12, x.diff_fn_0_1x)", &map),
            "m::f(diff_fn_0_12, x.diff_fn_0_1x)"
        );
    }
}
