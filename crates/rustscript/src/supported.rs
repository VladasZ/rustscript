//! `rust supported`: print the bridged method surface the binary actually
//! carries, straight from the tables the coverage harvest generated. The same
//! rendering produces `docs/supported.md`, and a test keeps that page in sync,
//! so neither view can drift from the dispatch source.

use crate::interpreter::coverage::{Avail, surface};

/// Receiver display names for the internal table keys.
fn recv_label(recv: &str) -> &str {
    match recv {
        "*" => "any value",
        "builtin" => "builtin (dispatched by id on matching receivers)",
        "Str" => "String and str",
        "Native" => "native handles (files, sockets, readers, processes)",
        "Enum" => "Option and Result (tokio mode)",
        other => other,
    }
}

fn mark(avail: Avail) -> &'static str {
    match avail {
        Avail::Both => "",
        Avail::FastOnly => " (fast)",
        Avail::ParallelOnly => " (tokio)",
    }
}

/// Group the surface by receiver, in table order.
fn groups() -> Vec<(&'static str, Vec<(&'static str, Avail)>)> {
    let mut out: Vec<(&'static str, Vec<(&'static str, Avail)>)> = Vec::new();
    for (recv, name, avail) in surface() {
        match out.last_mut() {
            Some((last, names)) if *last == recv => names.push((name, avail)),
            _ => out.push((recv, vec![(name, avail)])),
        }
    }
    out
}

/// The terminal listing.
pub fn print_supported() {
    println!(
        "Methods the interpreter implements, by receiver. A name marked (fast)\n\
         runs only on the single threaded engine; (tokio) only on the parallel\n\
         engine that #[tokio::main] selects. Unmarked names run on both.\n"
    );
    for (recv, names) in groups() {
        println!("{}:", recv_label(recv));
        let line: Vec<String> = names
            .iter()
            .map(|(n, a)| format!("{n}{}", mark(*a)))
            .collect();
        println!("  {}\n", line.join(", "));
    }
}

/// The markdown page committed as `docs/supported.md`.
pub fn markdown() -> String {
    let mut out = String::from(
        "# Supported interpreter surface\n\n\
         Generated from the bridge dispatch tables. Do not edit by hand; run\n\
         `rust supported md > docs/supported.md` after changing a bridge, and\n\
         the `supported_page_is_current` test enforces it.\n\n\
         A method marked `fast` runs only on the single threaded engine. One\n\
         marked `tokio` runs only on the parallel engine that `#[tokio::main]`\n\
         selects. Unmarked methods run on both.\n",
    );
    for (recv, names) in groups() {
        out.push_str(&format!("\n## {}\n\n", recv_label(recv)));
        let line: Vec<String> = names
            .iter()
            .map(|(n, a)| format!("`{n}`{}", mark(*a)))
            .collect();
        out.push_str(&line.join(", "));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed page must match what the tables render right now.
    /// Regenerate with `rust supported md > docs/supported.md`.
    #[test]
    fn supported_page_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/supported.md");
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            committed,
            markdown(),
            "docs/supported.md is stale, regenerate it with \
             `rust supported md > docs/supported.md`"
        );
    }
}
