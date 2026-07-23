//! Multifile script tests. The conformance test compiles the dedicated
//! `rustscript-conformance` crate with cargo, runs the binary, runs the same
//! `main.rs` through the interpreter, and asserts identical stdout. The rest
//! exercise the loader's error paths and import resolution on temp fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// A fresh temp directory populated with the given relative files.
fn fixture(files: &[(&str, &str)]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("rustscript_multifile_{}_{id}", std::process::id()));
    for (rel, content) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
    dir
}

fn run_script(path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("run")
        .arg(path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript")
}

#[test]
fn conformance_matches_compiler() {
    let root = workspace_root();
    // The compiled binary is looked up next to this test binary, so it has to be
    // built into the same profile. Without this a `cargo test --release` run
    // builds a debug binary and then goes looking for it in the release tree.
    let mut build_args = vec!["build", "-p", "rustscript-conformance"];
    if !cfg!(debug_assertions) {
        build_args.push("--release");
    }
    let build = Command::new(env!("CARGO"))
        .args(&build_args)
        .current_dir(&root)
        .status()
        .expect("failed to build conformance crate");
    assert!(
        build.success(),
        "cargo build -p rustscript-conformance failed"
    );

    // target/<profile>/deps/<testbin> -> target/<profile>/conformance
    let exe = std::env::current_exe().expect("current exe");
    let compiled = exe.parent().unwrap().parent().unwrap().join("conformance");
    let out = Command::new(&compiled)
        .output()
        .expect("failed to run compiled binary");
    assert!(
        out.status.success(),
        "compiled conformance binary exited with error"
    );

    let script = root.join("crates/conformance/src/main.rs");
    let interp = run_script(&script);
    assert!(
        interp.status.success(),
        "interpreted conformance failed:\n{}",
        String::from_utf8_lossy(&interp.stderr)
    );
    assert_eq!(
        out.stdout,
        interp.stdout,
        "output differs\n-- compiled --\n{}\n-- interpreted --\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&interp.stdout),
    );
}

#[test]
fn imports_of_structs_types_and_consts() {
    let dir = fixture(&[
        (
            "main.rs",
            r#"
mod a;
use a::b::{Thing, KIND as WHAT, Meters};
use a::make_default as build;

fn main() {
    let t = Thing { size: 4 };
    let d = build();
    let m: Meters = t.size + d.size;
    println!("{m} {WHAT} {:?}", d);
}
"#,
        ),
        (
            "a.rs",
            r#"
pub mod b;
use self::b::Thing;

pub fn make_default() -> Thing {
    Thing { size: 38 }
}
"#,
        ),
        (
            "a/b.rs",
            r#"
pub const KIND: &str = "thing";
pub type Meters = i64;

#[derive(Debug)]
pub struct Thing {
    pub size: i64,
}
"#,
        ),
    ]);
    let out = run_script(&dir.join("main.rs"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "42 thing Thing { size: 38 }\n"
    );
}

#[test]
fn reexport_chain_resolves() {
    let dir = fixture(&[
        (
            "main.rs",
            r#"
mod inner;
mod facade;
use facade::{Widget, WIDGET_NAME};

fn main() {
    let w = Widget::new(7);
    println!("{} {}", WIDGET_NAME, w.id);
}
"#,
        ),
        (
            "inner.rs",
            r#"
pub const WIDGET_NAME: &str = "widget";

pub struct Widget {
    pub id: i64,
}

impl Widget {
    pub fn new(id: i64) -> Widget {
        Widget { id }
    }
}
"#,
        ),
        (
            "facade.rs",
            r#"
pub use crate::inner::{Widget, WIDGET_NAME};
"#,
        ),
    ]);
    let out = run_script(&dir.join("main.rs"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "widget 7\n");
}

#[test]
fn path_attribute_points_at_an_explicit_file() {
    // A bin splits its modules into a subdirectory via #[path], and a #[path] module's own
    // submodules resolve relative to that file's directory, so the nested inner lands in sub/.
    let dir = fixture(&[
        (
            "main.rs",
            r#"
#[path = "sub/state.rs"]
mod state;
use state::greeting;

fn main() {
    println!("{}", greeting());
}
"#,
        ),
        (
            "sub/state.rs",
            r#"
#[path = "helpers/inner.rs"]
mod inner;

pub fn greeting() -> String {
    format!("hi {}", inner::who())
}
"#,
        ),
        (
            "sub/helpers/inner.rs",
            r#"
pub fn who() -> &'static str {
    "world"
}
"#,
        ),
    ]);
    let out = run_script(&dir.join("main.rs"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi world\n");
}

#[test]
fn missing_module_file_errors() {
    let dir = fixture(&[(
        "main.rs",
        r#"
mod nope;
fn main() {}
"#,
    )]);
    let out = run_script(&dir.join("main.rs"));
    assert!(!out.status.success(), "script unexpectedly succeeded");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("cannot find module `nope`"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("nope.rs"),
        "error should list tried paths: {err}"
    );
}

#[test]
fn glob_import_of_module_errors() {
    let dir = fixture(&[
        (
            "main.rs",
            r#"
mod util;
use util::*;

fn main() {
    println!("{}", helper());
}
"#,
        ),
        (
            "util.rs",
            r#"
pub fn helper() -> i64 { 1 }
"#,
        ),
    ]);
    let out = run_script(&dir.join("main.rs"));
    assert!(!out.status.success(), "script unexpectedly succeeded");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("glob import"), "unexpected error: {err}");
}
