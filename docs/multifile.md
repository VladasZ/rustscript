# Writing multifile scripts

A script can grow past one file using normal Rust module syntax. This guide
walks through doing it properly, from a single file to a small tree, and lists
the rules and the common mistakes.

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

When the file gets long, split it. Nothing about how you run it changes. You
still run the root file, and the root file pulls the rest in.

## The worked example

A word frequency report tool split into four files. The layout:

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

Run the root file. The other files are found through the `mod` declarations.

```
chmod +x report.rs
./report.rs notes.txt 3
```

## File layout rules

The rules are exactly the ones rustc uses.

- `mod name;` in the root file loads `name.rs` or `name/mod.rs` from the
  directory the root file sits in.
- `mod child;` inside `name.rs` loads `name/child.rs` or `name/child/mod.rs`.
  A module's children live in the directory named after it.
- Both styles work and mix freely. `name.rs` plus a `name/` directory for its
  children, or `name/mod.rs` with siblings in the same directory.
- Inline modules work too, `mod helpers { .. }` right in any file, nested as
  deep as you like.
- If both `name.rs` and `name/mod.rs` exist the script errors, same as rustc.

Only the root file gets a shebang. Module files are plain Rust source, they
are never run directly.

## Imports

All the normal forms work.

```rust
use crate::config::Config;          // absolute from the script root
use self::words::top_words;         // relative to the current module
use super::shared::helper;          // parent module
use stats::words::top_words;        // plain path, from the root file only
use config::Config as Cfg;          // rename
use stats::{self, words::top_words};  // groups and nested groups
```

Two things trip people up.

- In the root file a plain `use stats::words::X` works because `stats` is a
  top level module. Inside a submodule a plain path does not see its own
  children. Write `use self::words::X` there, or go absolute with
  `use crate::stats::words::X`.
- Glob imports of script modules are not supported. `use stats::*` stops with
  a clear error. Import the names you use, or re-export them under one roof.

Re-exports work and chain. A prelude style module is fine:

```rust
// prelude.rs
pub use crate::config::Config;
pub use crate::stats::words::top_words;
```

Then `use prelude::{Config, top_words};` from the root.

## What crosses file boundaries

Everything a script can define: functions, structs, tuple and unit structs,
enums, `impl` blocks, module level `const` and `static`, and type aliases.
An `impl` can live in the same file as its type and be called from anywhere.
Struct literals and tuple struct constructors work through a type alias.
Consts can reference consts from other modules in any order, they are
evaluated on first use.

Two modules can each define a type with the same name. `a::Config` and
`b::Config` are distinct types with their own methods.

## Visibility

Write `pub` where real Rust needs it, because `rust check` enforces it and a
missing `pub` fails that check. The interpreter itself does not check
visibility at runtime, the compiler stays the authority when you ask for it.

## The check gate and caching

The `cargo check` gate runs when you invoke `rust check`, not when a script
runs. It covers the whole file tree. The cache key hashes every file reachable
through `mod` declarations, so editing any module means the next `rust check`
rechecks, while an unchanged tree returns from cache at once. Running a script
never waits on the gate.

## Local crate dependencies

A script that lives inside a cargo crate can also pull in a local library crate
through a normal `path` dependency, not just its own `mod` files. This is how a
set of scripts shares one helper crate instead of copying modules around.

The interpreter reads the nearest `Cargo.toml` at or above the script, finds each
`[dependencies]` entry that points at a local `path`, and grafts that crate in.
Its `src/lib.rs` and the module tree below it load as a top level module named
after the crate, so `use shared::run::capture` resolves at runtime. The
`cargo check` gate adds the same crate as a real path dependency, so the editor
and clippy resolve it too. The dependency directory is pinned to an absolute path,
so the gate finds it no matter where it runs from.

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

The grafted crate can have its own multi-file module tree, loaded by the same
rules as a script's own modules. A change to any of its files re-triggers the
check for every script that uses it, since the cache key hashes the crate's
sources too.

## Not supported

- `#[path = "..."]` on a mod declaration.
- Glob imports of script modules, `use util::*`.
- `static mut`. A plain `static` behaves like a const.

Each one stops with a clear error instead of misbehaving.

## A bigger reference

The `crates/conformance` crate in this repo is a full multifile program that
exercises every import style, re-export chains, and cross module types,
consts, statics, and aliases. It compiles with cargo and runs under the
interpreter with identical output, so everything in it is known to work.
