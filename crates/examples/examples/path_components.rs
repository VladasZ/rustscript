#!/usr/bin/env rust

use std::path::{Component, Path, PathBuf};

fn main() {
    let path = Path::new("/a/b/./c/../d/.");

    for component in path.components() {
        let kind = match component {
            Component::Prefix(_) => "prefix",
            Component::RootDir => "root",
            Component::CurDir => "cur",
            Component::ParentDir => "parent",
            Component::Normal(_) => "normal",
        };
        println!("{kind} {}", component.as_os_str().to_string_lossy());
    }

    let clean: PathBuf = path.components().collect();
    println!("clean {}", clean.display());

    let turbofish = Path::new("x/./y").components().collect::<PathBuf>();
    println!("turbofish {}", turbofish.display());

    let names: Vec<String> = Path::new("./one/two")
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    println!("names {names:?}");

    let parents = Path::new("../../up")
        .components()
        .filter(|component| *component == Component::ParentDir)
        .count();
    println!("parents {parents}");
    println!("count {}", Path::new("/usr/local/bin").components().count());
}
