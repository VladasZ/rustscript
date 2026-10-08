#!/usr/bin/env rust

//! The `&mut self` methods of `PathBuf` change the path itself: `push` of a `&str`, a `String`,
//! a `Path` and a `Component`, `pop`, `set_extension` and `set_file_name`.

use std::path::{Component, Path, PathBuf};

struct Job {
    out: PathBuf,
}

/// The path from the folder `from` to `to`.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in shared..from.len() {
        path.push("..");
    }
    for part in &to[shared..] {
        path.push(part);
    }
    path
}

fn add_twice(path: &mut PathBuf, name: &str) {
    path.push(name);
    path.push(name);
}

fn main() {
    let mut a = PathBuf::new();
    a.push("x");
    println!("a `{}`", a.display());

    let mut b = PathBuf::from("x");
    b.push("y");
    println!("b `{}`", b.display());

    let mut c = PathBuf::from("x");
    c.push("..");
    println!("c `{}`", c.display());

    println!(
        "relative `{}`",
        relative(Path::new("a/b/c"), Path::new("a/d/e")).display()
    );
    println!(
        "relative same `{}`",
        relative(Path::new("a/b"), Path::new("a/b")).display()
    );

    let mut mixed = PathBuf::from("root");
    mixed.push(String::from("string"));
    mixed.push(Path::new("path"));
    mixed.push(PathBuf::from("buf"));
    mixed.push(Component::ParentDir);
    println!("mixed `{}`", mixed.display());

    let mut by_ref = PathBuf::from("one");
    add_twice(&mut by_ref, "two");
    println!("by ref `{}`", by_ref.display());

    let mut job = Job {
        out: PathBuf::from("target"),
    };
    job.out.push("debug");
    println!("field `{}`", job.out.display());

    let mut popped = PathBuf::from("a/b/c");
    let first = popped.pop();
    println!("pop `{}` {first}", popped.display());
    popped.pop();
    popped.pop();
    let last = popped.pop();
    println!("pop empty `{}` {last}", popped.display());

    let mut ext = PathBuf::from("dir/file.txt");
    let set = ext.set_extension("md");
    println!("extension `{}` {set}", ext.display());
    ext.set_extension("");
    println!("extension none `{}`", ext.display());
    let mut no_name = PathBuf::from("..");
    let set = no_name.set_extension("md");
    println!("extension no name `{}` {set}", no_name.display());

    let mut name = PathBuf::from("dir/file.txt");
    name.set_file_name("other.rs");
    println!("file name `{}`", name.display());

    // a change of one path never shows in the path it was made from
    let base = PathBuf::from("base");
    let mut copy = base.clone();
    copy.push("copy");
    let mut owned = base.as_path().to_path_buf();
    owned.push("owned");
    let mut from = PathBuf::from(&base);
    from.push("from");
    let mut joined = base.join("joined");
    joined.pop();
    joined.push("again");
    println!(
        "base `{}` copy `{}` owned `{}` from `{}` joined `{}`",
        base.display(),
        copy.display(),
        owned.display(),
        from.display(),
        joined.display()
    );
}
