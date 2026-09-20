//! `cfg` predicates, folded for the host the script runs on, like real Rust does at compile time.

use anyhow::{Result, bail};
use syn::punctuated::Punctuated;
use syn::{Expr, Lit};

/// False when any `#[cfg(..)]` on the item is false. 2 functions with the same name behind
/// `#[cfg(windows)]` and `#[cfg(not(windows))]` both used to load, and the later one won.
pub fn enabled(attrs: &[syn::Attribute]) -> Result<bool> {
    for attr in attrs {
        if !attr.path().is_ident("cfg") {
            continue;
        }
        if !eval(&attr.parse_args::<syn::Meta>()?)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Anything unhandled is an error, a silent false would pick the wrong branch.
pub fn eval(meta: &syn::Meta) -> Result<bool> {
    match meta {
        syn::Meta::Path(path) => {
            let name = path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match name.as_str() {
                "windows" => Ok(cfg!(windows)),
                "unix" => Ok(cfg!(unix)),
                "test" | "debug_assertions" | "doc" | "miri" => Ok(false),
                other => bail!("unsupported cfg predicate `{other}`"),
            }
        }
        syn::Meta::NameValue(nv) => {
            let key = nv
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let Expr::Lit(lit) = &nv.value else {
                bail!("cfg value must be a string literal");
            };
            let Lit::Str(want) = &lit.lit else {
                bail!("cfg value must be a string literal");
            };
            let want = want.value();
            Ok(match key.as_str() {
                "target_os" => want == std::env::consts::OS,
                "target_arch" => want == std::env::consts::ARCH,
                "target_family" => want == std::env::consts::FAMILY,
                "target_pointer_width" => want == (usize::BITS).to_string(),
                other => bail!("unsupported cfg key `{other}`"),
            })
        }
        syn::Meta::List(list) => {
            let op = list
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let inner: Punctuated<syn::Meta, syn::Token![,]> =
                list.parse_args_with(Punctuated::parse_terminated)?;
            let mut results = Vec::new();
            for m in &inner {
                results.push(eval(m)?);
            }
            match op.as_str() {
                "not" => match results.as_slice() {
                    [one] => Ok(!one),
                    _ => bail!("cfg not() takes exactly one predicate"),
                },
                "all" => Ok(results.iter().all(|r| *r)),
                "any" => Ok(results.iter().any(|r| *r)),
                other => bail!("unsupported cfg combinator `{other}`"),
            }
        }
    }
}
