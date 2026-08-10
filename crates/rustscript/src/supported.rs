//! `rust supported`: print the bridged method surface the binary actually
//! carries, straight from the tables the coverage harvest generated. The same
//! rendering produces `docs/supported.md`, and a test keeps that page in sync,
//! so neither view can drift from the dispatch source.

use crate::interpreter::coverage::surface;

/// Receiver display names for the internal table keys.
fn recv_label(recv: &str) -> &str {
    match recv {
        "*" => "any value",
        "builtin" => "builtin (dispatched by id on matching receivers)",
        "Str" => "String and str",
        "Native" => "native handles (files, sockets, readers, processes)",
        other => other,
    }
}

/// Group the surface by receiver, in table order.
fn groups() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut out: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for (recv, name) in surface() {
        match out.last_mut() {
            Some((last, names)) if *last == recv => names.push(name),
            _ => out.push((recv, vec![name])),
        }
    }
    out
}

/// The terminal listing.
pub fn print_supported() {
    println!("Methods the interpreter implements, by receiver.\n");
    for (recv, names) in groups() {
        println!("{}:", recv_label(recv));
        println!("  {}\n", names.join(", "));
    }
}

/// The markdown page committed as `docs/supported.md`.
pub fn markdown() -> String {
    let mut out = String::from(
        "# Supported interpreter surface\n\n\
         Generated from the bridge dispatch tables. Do not edit by hand; run\n\
         `rust supported md > docs/supported.md` after changing a bridge, and\n\
         the `supported_page_is_current` test enforces it.\n",
    );
    for (recv, names) in groups() {
        out.push_str(&format!("\n## {}\n\n", recv_label(recv)));
        let line: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
        out.push_str(&line.join(", "));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::read_to_string;

    /// The committed page must match what the tables render right now.
    /// Regenerate with `rust supported md > docs/supported.md`.
    #[test]
    fn supported_page_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/supported.md");
        let committed = read_to_string(path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert_eq!(
            committed,
            markdown(),
            "docs/supported.md is stale, regenerate it with \
             `rust supported md > docs/supported.md`"
        );
    }
}
