/// A script panic as a typed error, so `main` can print the header and exit 101.
#[derive(Debug)]
pub struct ScriptPanic {
    /// for the panic header
    pub file: String,
    pub line: u32,
    pub rendered: String,
}

impl std::fmt::Display for ScriptPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::error::Error for ScriptPanic {}

/// A compiled binary prints `Error: ...` and exits 1, so this is kept apart from a panic.
#[derive(Debug)]
pub struct ErrReturn(pub String);

impl std::fmt::Display for ErrReturn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error: {}", self.0)
    }
}

impl std::error::Error for ErrReturn {}

/// Frames arrive innermost first, line 0 means unknown. Deep chains are capped so runaway
/// recursion stays readable.
pub(super) fn trace_error(
    e: anyhow::Error,
    frames: impl Iterator<Item = (String, String, u32)>,
) -> anyhow::Error {
    const SHOWN: usize = 15;
    let mut msg = format!("{e:#}");
    // a closure inside a bridge wraps first, the panic origin must stay that innermost site
    let mut origin: Option<(String, u32)> = e
        .downcast_ref::<ScriptPanic>()
        .map(|p| (p.file.clone(), p.line));
    let mut hidden = 0usize;
    for (i, (func, file, line)) in frames.enumerate() {
        if origin.is_none() {
            origin = Some((file.clone(), line));
        }
        if i >= SHOWN {
            hidden += 1;
            continue;
        }
        if file.is_empty() {
            msg.push_str(&format!("\n  at {func}"));
        } else if line == 0 {
            msg.push_str(&format!("\n  at {func} ({file})"));
        } else {
            msg.push_str(&format!("\n  at {func} ({file}:{line})"));
        }
    }
    if hidden > 0 {
        msg.push_str(&format!("\n  ... {hidden} more frames"));
    }
    let (file, line) = origin.unwrap_or_default();
    anyhow::Error::new(ScriptPanic {
        file,
        line,
        rendered: msg,
    })
}
