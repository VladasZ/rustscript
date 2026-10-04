//! A log of what the VM really dispatched, one line per receiver kind and method, or per path,
//! the first time it runs. `RUSTSCRIPT_DISPATCH_LOG=FILE` turns it on. The differential harness
//! reads the files of a whole campaign, and what is in the bridge tables and in no file is what
//! no generated program reached.

use std::collections::HashSet;
use std::env::var_os;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use super::value::Value;

static ON: AtomicBool = AtomicBool::new(false);
static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

struct Log {
    file: File,
    seen: HashSet<String>,
}

/// Opens the log the environment names. A file that can't be opened leaves the log off.
pub fn init() {
    let Some(path) = var_os("RUSTSCRIPT_DISPATCH_LOG") else {
        return;
    };
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            if LOG
                .set(Mutex::new(Log {
                    file,
                    seen: HashSet::new(),
                }))
                .is_ok()
            {
                ON.store(true, Ordering::Relaxed);
            }
        }
        Err(e) => eprintln!("cannot open the dispatch log {}: {e}", path.display()),
    }
}

#[inline]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn method(recv: &Value, name: &str) {
    record(format!("method {} {name}", label(recv)));
}

/// `type_name` says `enum` and `native`, the bridge tables go by `Option` and `Iterator`.
fn label(recv: &Value) -> String {
    match recv {
        Value::Enum { def, .. } => def.name.to_string(),
        Value::Native(native) => native
            .try_lock()
            .map_or("native", |native| native.type_name())
            .to_string(),
        Value::Ref(reference) => reference
            .get()
            .map_or_else(|| "reference".to_string(), |value| label(&value)),
        other => other.type_name().to_string(),
    }
}

pub fn path(path: &str) {
    record(format!("path {path}"));
}

fn record(line: String) {
    let Some(log) = LOG.get() else {
        return;
    };
    let mut log = log.lock();
    if log.seen.contains(&line) {
        return;
    }
    if let Err(e) = writeln!(log.file, "{line}") {
        eprintln!("cannot write the dispatch log: {e}");
    }
    log.seen.insert(line);
}
