# Writing multifile scripts

A script can grow past one file with normal Rust module syntax. This guide
shows how, and lists the rules and common mistakes.

## Start with one file

Every script starts as a single file with `fn main`.

```rust
#!/usr/bin/env rust

fn main() -> anyhow::Result<()> {
    let text = std::fs::read_to_string("notes.txt")?;
    println!("{} words", text.split_whitespace().count());
    Ok(())
}
```

When the file gets long, split it. You still run the root file and it pulls
the rest in.

## The worked example

A word frequency tool split into 4 files.

```
report.rs        the root, has the shebang and fn main
config.rs        argument parsing
stats/mod.rs     a module directory
stats/words.rs   the counting logic
```

`report.rs`, the root:

```rust
#!/usr/bin/env rust

mod config;
mod stats;

use config::Config;
use stats::words::top_words;

fn main() -> anyhow::Result<()> {
    let cfg = Config::from_args();
    let text = std::fs::read_to_string(&cfg.path)?;
    for (word, n) in top_words(&text, cfg.limit) {
        println!("{n:>5} {word}");
    }
    Ok(())
}
```

`config.rs`:

```rust
pub struct Config {
    pub path: String,
    pub limit: usize,
}

impl Config {
    pub fn from_args() -> Config {
        let args: Vec<String> = std::env::args().collect();
        let path = args.get(1).cloned().unwrap_or("notes.txt".to_string());
        let limit = match args.get(2) {
            Some(n) => n.parse().unwrap_or(10),
            None => 10,
        };
        Config { path, limit }
    }
}
```

`stats/mod.rs`:

```rust
pub mod words;
```

`stats/words.rs`:

```rust
use std::collections::HashMap;

pub fn top_words(text: &str, limit: usize) -> Vec<(String, i64)> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for w in text.split_whitespace() {
        let n = counts.get(w).copied().unwrap_or(0) + 1;
        counts.insert(w.to_string(), n);
    }
    let mut pairs: Vec<(String, i64)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| if a.1 == b.1 { a.0.cmp(&b.0) } else { b.1.cmp(&a.1) });
    pairs.truncate(limit);
    pairs
}
```

Run the root file. The others are found through `mod`.

```
chmod +x report.rs
./report.rs notes.txt 3
```

## File layout rules

The rules are the ones `rustc` uses.

- `mod name;` loads `name.rs` or `name/mod.rs` next to the file.
- `mod child;` inside `name.rs` loads `name/child.rs` or `name/child/mod.rs`.
- Both styles mix freely.
- Inline `mod helpers { .. }` works at any depth.
- Both `name.rs` and `name/mod.rs` present is an error, same as `rustc`.
- `#[path = "dir/file.rs"]` on a `mod` picks the file. Its own `mod`
  declarations resolve relative to that file's directory, same as `rustc`.

Only the root file gets a shebang.

## Imports

All the normal forms work.

```rust
use crate::config::Config;          // absolute from the script root
use self::words::top_words;         // relative to the current module
use super::shared::helper;          // parent module
use stats::words::top_words;        // plain path, from the root file only
use config::Config as Cfg;          // rename
use stats::{self, words::top_words};  // groups and nested groups
use which::which;                   // a crate function named like its crate
```

2 things trip people up.

- A plain `use stats::words::X` works only in the root file. Inside a
  submodule write `use self::words::X` or `use crate::stats::words::X`.
- Glob imports of script modules like `use stats::*` are not supported.

Re-exports work and chain, so a prelude module is fine:

```rust
// prelude.rs
pub use crate::config::Config;
pub use crate::stats::words::top_words;
```

Then `use prelude::{Config, top_words};` from the root.

## What crosses file boundaries

Everything a script can define: functions, structs, enums, `impl` blocks,
`const`, `static` and type aliases. Consts can reference consts from other
modules in any order, they are evaluated on first use. `a::Config` and
`b::Config` are distinct types.

## Visibility

Write `pub` where real Rust needs it, `rust check` enforces it. The
interpreter does not check visibility at runtime.

## The check gate and caching

`rust check` covers the whole file tree. The cache key hashes every file
reachable through `mod`, so editing any module rechecks and an unchanged tree
hits the cache. Running a script never waits on the gate.

## Local crate dependencies

A script inside a cargo crate can use a local library crate through a normal
`path` dependency. This is how a set of scripts shares one helper crate.

The interpreter reads the nearest `Cargo.toml` above the script and grafts
every `path` dependency in as a top level module named after the crate, so
`use shared::run::capture` resolves. The `cargo check` gate adds the same
crate as a real path dependency, so clippy and the editor resolve it too.

For example, a `shared` crate next to the scripts:

```
tools/
  Cargo.toml         # dependencies: shared = { path = "shared" }
  shared/
    Cargo.toml       # package name = "shared"
    src/lib.rs       # pub mod run;  pub mod walk;
  src/bin/
    st.rs            # use shared::run::capture;
```

The grafted crate can have its own module tree. Its sources are part of the
cache key. Inside it `crate::` and `super::` resolve against its own root, so
its files mean the same thing interpreted and compiled.

## Not supported

- Glob imports of script modules, `use util::*`.
- `static mut`. A plain `static` behaves like a const.

Each one stops with a clear error.

## A bigger reference

`crates/conformance` is a full multifile program with every import style,
re-export chains and cross module items. It compiles with cargo and runs
under the interpreter with identical output.
