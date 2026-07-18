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

A script is a normal Rust program with `fn main`, one file or a module tree.
Two layers share the same source.

- An interpreter parses the files with [`syn`](https://github.com/dtolnay/syn),
  compiles them once to bytecode, and runs them on a register machine. Locals
  are numbered slots, so a variable read is an array index, not a name lookup.
  Ownership and borrow rules carry no meaning at runtime, so there is no borrow
  checker cost and startup is fast.
- `rust check` builds a small cargo project around the files and runs
  `cargo check` on it. This proves the script is valid Rust. It is a separate
  opt-in step, not part of running, and its result is cached by source hash.

Running never waits on a check, so a script starts at once, like Python. The
interpreter needs no type checker of its own. When you want proof that a script
is valid Rust, `rust check` makes the real compiler the authority. The
interpreter stays small and optimistic, and a bad path surfaces as a runtime
error when it is reached.

## Install

```
cargo install --path crates/rustscript
```

This installs a binary named `rust`.

## Usage

```
rust run FILE.rs     interpret the script
rust FILE.rs         same as run
rust FILE.rs cmp     compile and run, `cmp` first arg is reserved
rust build FILE.rs   compile to a native binary, cache it, then run
rust check FILE.rs   validate with cargo check, does not run
rust clean           clear the cache
rust update          install the latest RustScript from GitHub
rust --version       show version and build information
```

`rust update` is explicit. It compares the interpreter's embedded Git commit
with the default branch HEAD of `VladasZ/rustscript`. An exact match is a
no-op; otherwise it installs that HEAD with `cargo install`. On Windows it
moves the running executable aside before installation and restores it if the
update fails.

`rust --version` prints the package version, Git commit, UTC build time, and
Cargo build profile. A local build with tracked changes marks the commit as
dirty, for example:

```
rustscript 0.1.0 (4ea5a27-dirty, built 2026-07-18T09:21:09Z, release)
```

`rust build` compiles the script with cargo instead of interpreting it, then
runs the resulting binary and exits with its status. The binary is cached by
source hash under the cache dir, so an unchanged script runs again instantly
with no cargo call. The first build of a new or edited script is a real cargo
build, so it is slow, later runs are not. A successful build also proves the
script is valid Rust, so it doubles as a check. Use it for CPU heavy scripts
where native speed pays back the build cost. The one shared cargo target dir is
kept so an edit rebuilds only the script crate, but only the final binaries are
cached, never per script target dirs.

The word `cmp` as the first argument to a script is reserved. When you run
`FILE.rs cmp ...`, the interpreter compiles and runs the script the same way
`rust build` does, then passes the rest of the arguments on. This is what makes
a plain launcher give both modes for free. A command named `foo` interprets,
and `foo cmp` runs the compiled build, since the launcher forwards the words
unchanged. Because `cmp` is intercepted, a script must not use `cmp` as its own
first positional argument, it would never reach the script. Later arguments are
free, only the very first is reserved.

A `#!/usr/bin/env rust` first line lets a `.rs` file run on its own. A shebang
is legal Rust, so the file still passes `cargo check`. A symlink to a script
runs too, even without an `.rs` extension. The link is resolved first, so
module files are found next to the real source.

## Modules

A script can span multiple files with normal Rust module syntax. `mod name;`
loads `name.rs` or `name/mod.rs` next to the declaring file, following the same
directory rules as rustc, and inline `mod name { .. }` blocks work too. Modules
nest to any depth.

```rust
// tool.rs
mod util;
use util::math::add;

fn main() {
    println!("{}", add(2, 3));
}
```

```rust
// util.rs
pub mod math;
```

```rust
// util/math.rs
pub fn add(a: i64, b: i64) -> i64 { a + b }
```

Paths resolve like real Rust: `crate::`, `self::`, `super::`, plain, renamed,
and grouped imports, `use x::{self}`, and `pub use` re-export chains. Imported
structs, enums, functions, consts, statics, and type aliases all work across
files, including struct literals and tuple struct constructors through an
alias. Two modules can each define a type with the same name. When you run
`rust check` it covers the whole file tree, and a change to any module rechecks.
Visibility is not enforced at runtime, `rust check` is the authority when you
want it.

Not supported: `#[path]` on a mod declaration, and glob imports of script
modules like `use util::*`, both stop with a clear error.

A script inside a cargo crate can also `use` a local library crate declared as a
`path` dependency in the nearest `Cargo.toml`. The interpreter grafts that crate
in from source, and the `cargo check` gate treats it as a real path dependency,
so a set of scripts can share one helper crate.

See [docs/multifile.md](docs/multifile.md) for a proper guide, a worked
example, and the common mistakes.

## What works

- functions, recursion, `let` and `mut`, arithmetic, comparison, logical and
  bitwise operators, casts, and `T::from` / `T::try_from` numeric conversions
- `if`, `if let`, `while`, `loop`, `for` over ranges, vectors, maps, and chars,
  `match` with guards and patterns
- `struct`, `enum`, tuple structs, unit structs, `impl` methods and associated
  functions
- modules across files and inline, every import style, re-exports, module
  level `const` and `static`, and type aliases
- closures and the common iterator methods, `map`, `filter`, `fold`, `find`,
  `any`, `all`, `sort_by`, `sort_by_key`, `copied`, `cloned`, and more,
  including method paths like `ToString::to_string` passed as a function value
- `Vec`, `String`, `HashMap`, `Option`, `Result`, the `?` operator
- slicing with ranges, `&v[1..3]`, `&s[..n]`, and open ends like `v[1..]` in
  index position
- `format!` and `println!` with `{name}`, `{:?}`, width, and precision
- `matches!`, byte string literals `b"..."`, and `unsafe` blocks run their body
- `#[derive(...)]` is accepted, serialization is done by reflection

## Standard library subset

Scripts use plain `std`. The interpreter bridges the common parts.

- `std::fs`, read, write, create and remove dirs, copy, rename, `read_dir`,
  `canonicalize`, `metadata`, `symlink_metadata`, `read_link`, `File`,
  `OpenOptions`, and the platform `symlink`
- `std::io`, `stdin`, `stdout`, `stderr`, `Read`, `Write`, `BufReader`, `Seek`,
  `lines` reading, and `IsTerminal`
- `std::process::Command`, `output`, `status`, and `spawn` with `Stdio` piping
  and a `Child` you can stream, feed, and `wait` on
- `std::net`, blocking `TcpListener` and `TcpStream`
- `std::time`, `Instant`, `SystemTime`, and `Duration`
- `std::env`, real script args, `vars`, `var`, `var_os`, `set_var`,
  `remove_var`, `current_dir`, `set_current_dir`, `temp_dir`, and
  `consts::OS` / `ARCH`
- `std::process::exit`
- `std::path`, `Path` and `PathBuf` with `display`, `is_dir`, `join`,
  `ancestors`, and more
- `std::collections`, `HashMap`, `BTreeMap`, sets, and the `entry` API

## Bridged crates

A script may declare real dependencies. A crate runs only if the interpreter has
a native bridge for it. These are bridged today.

- [`serde`](https://serde.rs) and
  [`serde_json`](https://github.com/serde-rs/json), including typed
  `from_str::<T>` into your own structs, with `#[serde(rename = "..")]` and
  `Option<T>` fields honored, so camelCase APIs map onto snake_case fields
- [`anyhow`](https://github.com/dtolnay/anyhow) for `Result`, `?`, `bail!`,
  `ensure!`, and `context`
- [`reqwest`](https://github.com/seanmonstar/reqwest) for HTTP and HTTPS over
  rustls, the blocking API in a plain script and the async API under
  `#[tokio::main]`, with headers, query params, json bodies, a timeout, and
  cookies
- [`regex`](https://github.com/rust-lang/regex) for matching, capture groups,
  and replace
- [`which`](https://github.com/harshadgavali/which-rs) to find a program on PATH
- [`glob`](https://github.com/rust-lang/glob) for path matching
- [`dirs`](https://github.com/dirs-dev/dirs-rs) for home, cache, and config dirs
- [`chrono`](https://github.com/chronotope/chrono) for `Utc::now`, formatting,
  and date parts
- [`rand`](https://github.com/rust-random/rand) for random numbers and bytes
- [`toml`](https://github.com/toml-rs/toml) and
  [`serde_yaml`](https://github.com/dtolnay/serde-yaml) for typed config
- [`colored`](https://github.com/colored-rs/colored) for terminal colors
- [`base64`](https://github.com/marshallpierce/base64) and
  [`hex`](https://github.com/KokaKiwi/rust-hex) for encoding
- [`ctrlc`](https://github.com/Detegr/rust-ctrlc) for a Ctrl-C handler
- [`tempfile`](https://github.com/Stebalien/tempfile) for temp dirs, files, and
  `NamedTempFile`
- [`jsonwebtoken`](https://github.com/Keats/jsonwebtoken) for signing JWTs,
  `Header`, `EncodingKey::from_ec_pem` and `from_secret`, and `encode`, so
  ES256 and HS256 tokens work

A crate without a bridge still passes `cargo check` but stops the interpreter
with `unsupported crate` when its code runs.

## Async and parallelism

A script whose `main` is `#[tokio::main]` runs on a second engine built for real
multi-core work. It uses a multi-thread tokio runtime, so `tokio::spawn` tasks
run on many threads at once, `.await`, `tokio::join!`, and
`tokio::task::yield_now` work, `tokio::time::sleep` is real, and the async
`reqwest` client sends requests concurrently. A plain script with no
`#[tokio::main]` keeps the fast single-thread engine untouched, so it pays
nothing for this. The `current_thread` flavor is rejected, only the multi-thread
runtime is offered.

## Not supported

`std::thread` is rejected, use `tokio::spawn` under `#[tokio::main]` for real
parallelism. `unsafe` blocks run their body, since edition 2024 needs `unsafe`
around calls like `env::set_var`. Lifetimes and generics parse and run, they
just carry no meaning at runtime. `static mut` is rejected, plain `static`
behaves like a const.

## Caching

Check results, compiled binaries, and the prebuilt dependencies live in
`~/.cache/rustscript`. Interpreting a script never touches this cache. When you
run `rust check`, the fixed dependency set compiles once into a shared target,
and each script's result is cached by source hash so an unchanged script
rechecks instantly. `rust build` shares that target and adds the finished
binary to a `bin` folder keyed by the same hash, so a rerun of an unchanged
script skips cargo. `rust clean` clears the cache.

## Examples

The scripts in `crates/examples/examples` cover the common ground people use to
judge a scripting language. Fizzbuzz, fibonacci, word count, quicksort, sieve,
towers of hanoi, roman numerals, a state machine, file and directory work, a
shell command, json config, typed json, an http fetch, and regex extraction.
Newer ones show process spawning with streamed output, file and stdin I/O, file
metadata and symlinks, tcp sockets, threads, dates, temp dirs, base64 and hex,
toml and yaml config, terminal colors, and running a program from PATH.

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
cargo test --test multifile             module loading, imports, and conformance
cargo test --test check -- --ignored    the cargo check gate, valid and invalid
```

The equivalence suite runs every example both as a compiled cargo binary and
through the interpreter, then checks the output matches byte for byte. It is the
strongest guarantee that the interpreter behaves like the real compiler. The
multifile suite does the same for the `crates/conformance` crate, a deep module
tree that exercises every import style, re-export chains, and cross module
types, consts, and aliases.

## Benchmarks

The `bench` crate compares rustscript against native Rust, Node, and Python 3 on
equivalent idiomatic tasks with byte-identical output. It records interleaved
wall-clock, self-timed compute, and peak-memory samples at two sizes, retains the
raw data and provenance, and draws one PNG per case and tier. See
`bench/README.md` for the methodology, current scope limits, and result format,
and `docs/profiling.md` for how to find interpreter hot spots.

```
cargo run --release --bin bench
cargo run --release --bin chart
```

## Status

Early but usable. Script arguments, `std::io::stdin`, process spawning, files,
sockets, and time all work now. Typed deserialization honors
`#[serde(rename = "..")]` and `Option<T>` fields. Known refinements still open
are container-level `#[serde(rename_all = "..")]` and `#[serde(default)]`.
