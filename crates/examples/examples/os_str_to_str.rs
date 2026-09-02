// `Path::extension` and `Path::file_stem` hand back values the interpreter models as plain
// strings, so `to_str` must return `Some` and `to_string_lossy` the text itself.

use std::path::Path;

fn main() {
    let path = Path::new("session.jsonl");

    let ext = path.extension().and_then(|ext| ext.to_str());
    println!("{ext:?}");

    let stem = path.file_stem().and_then(|stem| stem.to_str());
    println!("{stem:?}");

    let lossy = path
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned());
    println!("{lossy:?}");

    let none = Path::new("noext").extension().and_then(|ext| ext.to_str());
    println!("{none:?}");
}
