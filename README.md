# rustscript

Run a subset of Rust as an interpreted script. Write helper and automation
scripts in real Rust and run them like Python or a shell script, without waiting
for a full compile.

```rust
#!/usr/bin/env rust

use std::fs;

fn main() -> anyhow::Result<()> {
    let text = fs::read_to_string("notes.txt")?;
    println!("{} lines", text.lines().count());
    Ok(())
}
```

Make it executable and run it directly.

```
chmod +x notes.rs
./notes.rs
```

## The idea

A script is a normal single file Rust program with `fn main`. Two layers share
that same source.

- An interpreter parses the file with [`syn`](https://github.com/dtolnay/syn)
  and walks the tree. Ownership and borrow rules carry no meaning at runtime, so
  there is no borrow checker cost and startup is fast.
- Before running, `rustscript` builds a small cargo project around the file and
  runs `cargo check` on it. This proves the file is valid Rust. The check is
  cached by source hash, so an unchanged script skips it.

The interpreter needs no type checker of its own. The real Rust compiler stays
the authority on whether a script is valid, so the interpreter can stay small
and optimistic.

## Install

```
cargo install --path crates/rustscript
```

This installs a binary named `rust`.

## Usage

```
rust run FILE.rs     check then interpret
rust FILE.rs         same as run
rust check FILE.rs   validate with cargo check only
rust clean           clear the check cache
```

A `#!/usr/bin/env rust` first line lets a `.rs` file run on its own. A shebang
is legal Rust, so the file still passes `cargo check`.

## What works

- functions, recursion, `let` and `mut`, arithmetic, comparison, logical and
  bitwise operators, casts
- `if`, `if let`, `while`, `loop`, `for` over ranges, vectors, maps, and chars,
  `match` with guards and patterns
- `struct`, `enum`, tuple structs, `impl` methods and associated functions
- closures and the common iterator methods, `map`, `filter`, `fold`, `find`,
  `any`, `all`, `sort_by`, `sort_by_key`, and more
- `Vec`, `String`, `HashMap`, `Option`, `Result`, the `?` operator
- `format!` and `println!` with `{name}`, `{:?}`, width, and precision
- `#[derive(...)]` is accepted, serialization is done by reflection

## Standard library subset

Scripts use plain `std`. The interpreter bridges the common parts.

- `std::fs`, read, write, create and remove dirs, copy, rename, `read_dir`,
  `canonicalize`
- `std::process::Command`, build, run, read `stdout`, `stderr`, and status
- `std::env`, args, vars, current dir
- `std::path`, `Path` and `PathBuf` with `display`, `is_dir`, `join`, and more
- `std::collections`, `HashMap`, `BTreeMap`, sets

## Bridged crates

A script may declare real dependencies. A crate runs only if the interpreter has
a native bridge for it. These are bridged today.

- [`serde`](https://serde.rs) and
  [`serde_json`](https://github.com/serde-rs/json), including typed
  `from_str::<T>` into your own structs
- [`anyhow`](https://github.com/dtolnay/anyhow) for `Result`, `?`, `bail!`,
  `ensure!`, and `context`
- [`ureq`](https://github.com/algesten/ureq) for HTTP and HTTPS over rustls
- [`regex`](https://github.com/rust-lang/regex) for matching, capture groups,
  and replace

A crate without a bridge still passes `cargo check` but stops the interpreter
with `unsupported crate` when its code runs.

## Not supported

`async`, threads, and `unsafe` are not run. Reaching them is a clean runtime
error, not a wrong result. Lifetimes and generics parse and run, they just carry
no meaning at runtime.

## Caching

Check results and the prebuilt dependencies live in `~/.cache/rustscript`, keyed
by source hash. The first run of a new or changed script pays the `cargo check`
gate once, later runs skip it. `rustscript clean` clears the cache.

## Examples

The scripts in `crates/examples/examples` cover the common ground people use to
judge a scripting language. Fizzbuzz, fibonacci, word count, quicksort, sieve,
towers of hanoi, roman numerals, a state machine, file and directory work, a
shell command, json config, typed json, an http fetch, and regex extraction.

Run one with the interpreter.

```
rust run crates/examples/examples/word_count.rs
```

Compile all of them with the real toolchain as a second check.

```
cargo build --examples -p rustscript-examples
```

## Tests

```
cargo test                              all suites, see below
cargo test --test run                   interpreter behavior
cargo test --test equivalence           compiled example vs interpreted, byte identical
cargo test --test check -- --ignored    the cargo check gate, valid and invalid
```

The equivalence suite runs every example both as a compiled cargo binary and
through the interpreter, then checks the output matches byte for byte. It is the
strongest guarantee that the interpreter behaves like the real compiler.

## Status

Early but usable. Known refinements still open are serde field attributes like
`rename_all` and `default`, `std::io::stdin`, and passing real script arguments
into `std::env::args`.
