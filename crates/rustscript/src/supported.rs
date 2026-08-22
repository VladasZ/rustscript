//! `rust supported` and `docs/supported.md`, both from the harvested tables.
//! A test keeps the page in sync.

use crate::interpreter::coverage::surface;

fn recv_label(recv: &str) -> &str {
    match recv {
        "*" => "any value",
        "builtin" => "builtin (dispatched by id on matching receivers)",
        "Str" => "String and str",
        "Native" => "native handles (files, sockets, readers, processes)",
        other => other,
    }
}

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

pub fn print_supported() {
    println!("Methods the interpreter implements, by receiver.\n");
    for (recv, names) in groups() {
        println!("{}:", recv_label(recv));
        println!("  {}\n", names.join(", "));
    }
}

pub fn markdown() -> String {
    let mut out = String::from(
        "# Supported interpreter surface\n\n\
         Generated from the bridge tables, do not edit by hand. Run\n\
         `rust supported md > docs/supported.md` after changing a bridge.\n\
         The `supported_page_is_current` test enforces it.\n",
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
