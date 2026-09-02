//! Keeps every source file under the size cap, so a module is split before it grows unreadable.

use std::fs::{read_dir, read_to_string};
use std::path::{Path, PathBuf};

const CAP: usize = 800;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn visit(dir: &Path, over: &mut Vec<String>) {
    for entry in read_dir(dir).expect("readable directory") {
        let path = entry.expect("directory entry").path();
        let name = path
            .file_name()
            .expect("a directory entry has a name")
            .to_string_lossy()
            .into_owned();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        // the one benchmark case whose point is being a large script
        if name == "big_script" {
            continue;
        }
        if path.is_dir() {
            visit(&path, over);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let lines = read_to_string(&path)
            .expect("readable source file")
            .lines()
            .count();
        if lines > CAP {
            over.push(format!("{} has {lines} lines", path.display()));
        }
    }
}

#[test]
fn source_files_stay_under_the_cap() {
    let mut over = Vec::new();
    visit(&workspace_root(), &mut over);
    assert!(
        over.is_empty(),
        "files over {CAP} lines, split them into focused modules:\n{}",
        over.join("\n")
    );
}
