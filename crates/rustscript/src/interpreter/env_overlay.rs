//! The environment a script sees. `env::set_var` and `env::remove_var` write here and never
//! touch the real process environment, which is not safe to change while other threads read
//! it. Reads look here first, and a spawned child gets the same view.
//!
//! The bridges that read a variable on the script's behalf read it here too: `PATH` for
//! `which`, `HOME` for `dirs::home_dir` and the temp directory variables for `env::temp_dir`
//! and `tempfile`. A crate that reads the real environment on its own, like the proxy
//! variables of `reqwest`, does not see a script's changes.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::LazyLock;

use parking_lot::Mutex;

/// `Some` is set by the script, `None` is removed by it, a missing key reads the real one.
static OVERLAY: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn set(key: String, value: String) {
    OVERLAY.lock().insert(key, Some(value));
}

pub(super) fn remove(key: String) {
    OVERLAY.lock().insert(key, None);
}

pub(super) fn var(key: &str) -> Result<String, std::env::VarError> {
    match OVERLAY.lock().get(key) {
        Some(Some(value)) => Ok(value.clone()),
        Some(None) => Err(std::env::VarError::NotPresent),
        None => std::env::var(key),
    }
}

pub(super) fn var_os(key: &str) -> Option<OsString> {
    match OVERLAY.lock().get(key) {
        Some(value) => value.clone().map(OsString::from),
        None => std::env::var_os(key),
    }
}

/// The real variables with the script's changes applied, new ones at the end.
pub(super) fn vars() -> Vec<(String, String)> {
    let overlay = OVERLAY.lock().clone();
    let mut out: Vec<(String, String)> = std::env::vars()
        .filter_map(|(k, v)| match overlay.get(&k) {
            Some(Some(changed)) => Some((k, changed.clone())),
            Some(None) => None,
            None => Some((k, v)),
        })
        .collect();
    let mut added: Vec<(String, String)> = overlay
        .iter()
        .filter_map(|(k, v)| {
            let value = v.as_ref()?;
            std::env::var_os(k)
                .is_none()
                .then(|| (k.clone(), value.clone()))
        })
        .collect();
    added.sort();
    out.extend(added);
    out
}

/// A child starts from the environment the script sees.
pub(super) fn apply(cmd: &mut std::process::Command) {
    for (key, value) in OVERLAY.lock().iter() {
        match value {
            Some(v) => {
                cmd.env(key, v);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
}

/// Whether the script changed any of these variables.
fn touched(keys: &[&str]) -> bool {
    let overlay = OVERLAY.lock();
    keys.iter().any(|k| overlay.contains_key(*k))
}

/// `env::temp_dir`, from the variables the platform reads.
pub(super) fn temp_dir() -> PathBuf {
    let keys: &[&str] = if cfg!(windows) {
        &["TMP", "TEMP", "USERPROFILE"]
    } else {
        &["TMPDIR"]
    };
    if !touched(keys) {
        return std::env::temp_dir();
    }
    keys.iter()
        .find_map(|k| var_os(k).filter(|v| !v.is_empty()))
        .map_or_else(
            || {
                if cfg!(windows) {
                    PathBuf::from(r"C:\Windows")
                } else {
                    PathBuf::from("/tmp")
                }
            },
            PathBuf::from,
        )
}

/// The directory `tempfile` creates in, `None` when the script left the variables alone.
pub(super) fn tempfile_dir() -> Option<PathBuf> {
    let keys: &[&str] = if cfg!(windows) {
        &["TMP", "TEMP", "USERPROFILE"]
    } else {
        &["TMPDIR"]
    };
    touched(keys).then(temp_dir)
}

/// `dirs::home_dir` reads `HOME` on unix, Windows asks the shell for the profile folder.
pub(super) fn home_dir() -> Option<PathBuf> {
    if cfg!(unix) && touched(&["HOME"]) {
        return var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    }
    dirs::home_dir()
}

/// The `PATH` for `which`, `None` when the script left it alone. A removed `PATH` finds
/// nothing.
pub(super) fn search_path() -> Option<OsString> {
    touched(&["PATH"]).then(|| var_os("PATH").unwrap_or_default())
}
