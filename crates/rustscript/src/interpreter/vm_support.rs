//! The script panic, its header in the exact form `rustc` prints, and the error trace.

/// A script panic as a typed error, so `main` can print the header and exit 101.
#[derive(Debug)]
pub struct ScriptPanic {
    /// for the panic header, empty when the site is unknown
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// the panic message alone
    pub message: String,
    /// the script frames, innermost first, shown under `RUST_BACKTRACE`
    pub trace: String,
}

impl ScriptPanic {
    /// What a compiled binary prints to stderr, same bytes except the thread id.
    pub fn header(&self, thread: &str) -> String {
        let tid = os_thread_id();
        let mut out = if self.file.is_empty() {
            format!("\nthread '{thread}' ({tid}) panicked:\n{}\n", self.message)
        } else {
            format!(
                "\nthread '{thread}' ({tid}) panicked at {}:{}:{}:\n{}\n",
                self.file, self.line, self.col, self.message
            )
        };
        if std::env::var_os("RUST_BACKTRACE").is_some_and(|v| v != "0") {
            out.push_str("stack backtrace:\n");
            out.push_str(&self.trace);
            out.push('\n');
        } else {
            out.push_str(
                "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n",
            );
        }
        out
    }
}

impl std::fmt::Display for ScriptPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScriptPanic {}

/// The id the real panic header shows, the OS thread id where the platform has one.
pub fn os_thread_id() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut id: u64 = 0;
        // SAFETY: `pthread_threadid_np` writes one u64 through a valid pointer and reads no
        // other memory, 0 asks for the calling thread.
        let rc = unsafe { libc::pthread_threadid_np(0, &raw mut id) };
        if rc == 0 { id } else { 0 }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: `gettid` takes no arguments and only reads the calling thread's id.
        let id = unsafe { libc::gettid() };
        u64::try_from(id).unwrap_or(0)
    }
    #[cfg(windows)]
    {
        // SAFETY: `GetCurrentThreadId` takes no arguments and cannot fail.
        u64::from(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() })
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        windows
    )))]
    {
        0
    }
}

/// A compiled binary prints `Error: ...` and exits 1, so this is kept apart from a panic.
#[derive(Debug)]
pub struct ErrReturn(pub String);

impl std::fmt::Display for ErrReturn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error: {}", self.0)
    }
}

impl std::error::Error for ErrReturn {}

/// One frame of the script call chain at the time of a panic.
pub struct FrameSite {
    pub func: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// Frames arrive innermost first, line 0 means unknown. Deep chains are capped so runaway
/// recursion stays readable.
pub(super) fn trace_error(
    e: anyhow::Error,
    frames: impl Iterator<Item = FrameSite>,
) -> anyhow::Error {
    const SHOWN: usize = 15;
    // a closure inside a bridge wraps first, the panic origin must stay that innermost site
    let (message, mut origin, mut trace) = match e.downcast_ref::<ScriptPanic>() {
        Some(p) => (
            p.message.clone(),
            Some((p.file.clone(), p.line, p.col)),
            p.trace.clone(),
        ),
        None => (format!("{e:#}"), None, String::new()),
    };
    let mut hidden = 0usize;
    for (i, site) in frames.enumerate() {
        if origin.is_none() {
            origin = Some((site.file.clone(), site.line, site.col));
        }
        if i >= SHOWN {
            hidden += 1;
            continue;
        }
        if !trace.is_empty() {
            trace.push('\n');
        }
        if site.file.is_empty() {
            trace.push_str(&format!("  at {}", site.func));
        } else if site.line == 0 {
            trace.push_str(&format!("  at {} ({})", site.func, site.file));
        } else {
            trace.push_str(&format!(
                "  at {} ({}:{}:{})",
                site.func, site.file, site.line, site.col
            ));
        }
    }
    if hidden > 0 {
        trace.push_str(&format!("\n  ... {hidden} more frames"));
    }
    let (file, line, col) = origin.unwrap_or_default();
    anyhow::Error::new(ScriptPanic {
        file,
        line,
        col,
        message,
        trace,
    })
}
