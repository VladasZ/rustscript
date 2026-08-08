# RustScript

[![Crates.io](https://img.shields.io/crates/v/run-rs.svg)](https://crates.io/crates/run-rs)
[![CI](https://github.com/VladasZ/rustscript/actions/workflows/ci.yml/badge.svg)](https://github.com/VladasZ/rustscript/actions/workflows/ci.yml)
[![Marketplace](https://img.shields.io/badge/marketplace-rustscript--action-2088FF?logo=githubactions&logoColor=white)](https://github.com/marketplace/actions/rustscript-action)
[![Licence](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue)](#licence)

[![Linux](https://img.shields.io/badge/linux-x86__64%20%7C%20arm64-informational?logo=linux&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)
[![macOS](https://img.shields.io/badge/macos-universal-informational?logo=apple&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)
[![Windows](https://img.shields.io/badge/windows-x86__64%20%7C%20arm64-informational?logo=windows&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)

Run helper and automation scripts in Rust without waiting for a full compile.
RustScript interprets a practical subset of the language. `rust check`
validates the same files with rustc.

## Install

Install the [`run-rs`](https://crates.io/crates/run-rs) package from crates.io:

```sh
cargo install run-rs
```

This installs a binary named `rust`.

## First script

```rust
#!/usr/bin/env rust

use std::fs;

fn main() -> anyhow::Result<()> {
    let text = fs::read_to_string("notes.txt")?;
    println!("{} lines", text.lines().count());
    Ok(())
}
```

Make the file executable and run it directly:

```sh
chmod +x notes.rs
./notes.rs
```

## Usage

```text
rust FILE.rs         interpret the script
rust run FILE.rs     same as above
rust -e 'CODE'       run a snippet, arguments after CODE go to it
rust check FILE.rs   validate without running
rust build FILE.rs   compile, cache, and run a native binary
rust supported       list every bridged method per receiver and engine
rust clean           clear cached checks and builds
rust update [VER]    install a release, the newest one by default
rust --version       show version and build information
```

Arguments after the file are passed to the script. The first argument `cmp` is
reserved as a shorthand for compiled mode:

```sh
rust tool.rs one two
rust tool.rs cmp one two
```

The shebang is valid Rust, so the same file can still be compiled or checked by
Cargo. Symlinks to scripts work too, including extensionless command names.

## How it works

- `rust FILE.rs` parses the source with
  [`syn`](https://github.com/dtolnay/syn), compiles it to bytecode, and runs it
  on a register VM. It does not invoke Cargo or a type checker.
- `rust check FILE.rs` creates a small Cargo project and runs `cargo check`.
  It then inspects every compiled branch for method calls the interpreter does
  not implement. Results are cached by source hash.
- `rust build FILE.rs` asks Cargo for a native binary, caches it, and runs it.
  Use it for CPU-heavy scripts that justify the initial build.

rustc remains responsible for type, ownership, borrowing, and visibility
errors. The interpreter does not implement a second Rust type system.

Failures behave like compiled Rust: a runtime abort prints a panic header
with the failing file and line plus a script backtrace and exits 101, and an
`Err` out of `main` prints `Error: ...` and exits 1.

Runtime numerics match a default `cargo run`, which is debug Rust. Values
carry their real width at runtime, u8 through u64, usize, i8 through i64,
f32, and f64, and u64 and usize keep their full range past i64::MAX.
Integer overflow on arithmetic, shifts, and negation panics exactly where
compiled Rust panics, a narrowing `as` cast truncates, a float to integer
cast saturates with NaN going to zero, and f32 computes and prints at f32
precision.

Integer methods answer in the receiver's own width too, so `saturating_add`
stops at that width's bound, `wrapping_add` wraps at it, `checked_add` reports
overflow against it, and `pow` and `abs` panic where debug Rust panics. The bit
methods count over the real width rather than over an i64.

Where a method's result type is chosen by the caller rather than by the
receiver, the interpreter honors what the source states. `parse` takes its
target from the turbofish, so `"300".parse::<u8>()` is an `Err` and a trailing
space is not part of a number. `sum` takes its element type the same way, which
is what tells an empty `sum::<f64>()` from an empty `sum::<i32>()`.
`unwrap_or_default` builds its default from the payload the call site names,
whether that is a `None::<T>`, the binding's own annotation, or the argument
that built the Option.

## Supported Rust

Supported language features include:

- functions, recursion, closures, methods, associated functions, and aliases
- structs, tuple structs, enums, patterns, guards, `if let`, and `let else`
- loops, ranges, arithmetic, comparison, casts, and bitwise operations
- `Vec`, strings, maps, sets, `Option`, `Result`, and `?`
- iterators including mutable iteration, `map`, `filter`, `fold`, `find`,
  sorting, and predicates
- formatting, named arguments, width, precision, and common macros
- modules, imports, re-exports, constants, statics, and local path crates
- `#[tokio::main]`, spawned tasks, joins, yielding, timers, and async HTTP
- typed and dynamic `serde_json`, `regex`, and `chrono` in tokio mode too

The standard-library bridge covers files, directories, paths, stdin and stdout,
buffered I/O, processes, TCP sockets, environment variables, arguments, time,
and collections.

The following crates have native interpreter bridges:

- [`anyhow`](https://github.com/dtolnay/anyhow),
  [`serde`](https://serde.rs), and
  [`serde_json`](https://github.com/serde-rs/json)
- [`reqwest`](https://github.com/seanmonstar/reqwest),
  [`regex`](https://github.com/rust-lang/regex),
  [`jsonwebtoken`](https://github.com/Keats/jsonwebtoken), and
  [`tokio`](https://tokio.rs)
- [`chrono`](https://github.com/chronotope/chrono),
  [`rand`](https://github.com/rust-random/rand),
  [`which`](https://github.com/harshadgavali/which-rs),
  [`glob`](https://github.com/rust-lang/glob), and
  [`dirs`](https://github.com/dirs-dev/dirs-rs)
- [`toml`](https://github.com/toml-rs/toml),
  [`serde_yaml`](https://github.com/dtolnay/serde-yaml),
  [`base64`](https://github.com/marshallpierce/base64),
  [`hex`](https://github.com/KokaKiwi/rust-hex), and
  [`colored`](https://github.com/colored-rs/colored)
- [`ctrlc`](https://github.com/Detegr/rust-ctrlc) and
  [`tempfile`](https://github.com/Stebalien/tempfile)
- [`lopdf`](https://github.com/J-F-Liu/lopdf) and
  [`xmltree`](https://github.com/eminence/xmltree-rs)
- [`ratatui`](https://github.com/ratatui/ratatui), the widget and buffer side.
  A script builds a `Table`, `Block` or `Sparkline`, renders it into a
  `Buffer`, and reads the cells back. There is no backend and no `Terminal`,
  so drawing works in a pipe and in CI.
- [`crossterm`](https://github.com/crossterm-rs/crossterm) for
  `terminal::size`, and
  [`terminal-light`](https://github.com/Canop/terminal-light) for `luma`, the
  brightness of the terminal background from 0 for black to 1 for white. A
  script that draws needs both, one to know whether its drawing fits the
  window, the other to pick colors the terminal can actually show. Neither
  answer is available in a pipe or in CI, so both report an error there.

Windows builds also bridge
[`winreg`](https://github.com/gentoo90/winreg-rs),
[`windows-service`](https://github.com/mullvad/windows-service-rs), and
[`wmi`](https://github.com/ohadravid/wmi-rs).

See the programs under `crates/examples/examples` for working examples of the
language, standard library, and crate bridges, and
[docs/supported.md](docs/supported.md) for the full generated list of bridged
methods per receiver and engine.

## Modules and local crates

Normal module layouts work: `mod name;` loads `name.rs` or `name/mod.rs`, and
modules can nest to any depth. Imports support `crate::`, `self::`, `super::`,
renames, groups, and re-export chains.

A script inside a Cargo project can use local library crates declared as path
dependencies in the nearest `Cargo.toml`. Both the interpreter and `rust check`
load the same source tree.

See [Writing multifile scripts](docs/multifile.md) for layout rules, a complete
example, and the unsupported module forms.

## Current limitations

`cargo check` proves that a program is valid Rust, not that every operation has
an interpreter bridge. `rust check` adds that coverage pass.

- Crates without a native bridge stop with an `unsupported crate` error.
- Coverage currently checks methods, not path calls such as
  `std::process::exit`.
- Glob imports from script modules are not supported.
- `std::thread` is not supported; use Tokio tasks for parallel work.
- `static mut` is rejected. Plain statics behave like constants.
- `u128` and `i128` carry no runtime width, their values compute in i64.
- Lifetimes and generics are accepted but carry no runtime meaning.
- Serde container attributes beyond `rename_all`, such as `default`, are not
  yet implemented by the reflection bridge.

## Caching

Checks, compiled binaries, and shared Cargo dependencies live under
`~/.cache/rustscript`, or the platform's own per-user cache directory when
`HOME` and `XDG_CACHE_HOME` are both unset. Interpreted runs do not touch the
cache. A cache entry is keyed by the sources, the bundled dependency set, and
the interpreter version, so an update never serves an answer the previous build
gave. Entries unused for 30 days are swept automatically on every check and
build, and `rust clean` removes everything at once.

## GitHub Actions

The repository is also a GitHub Action:

```yaml
- uses: VladasZ/rustscript@v0.2
  with:
    script: tools/release.rs
    args: --dry-run
```

The Action downloads a checksum-verified prebuilt binary, so setup takes
seconds instead of compiling the crate. It supports Linux, macOS, and Windows
on x86_64 and arm64. See the [GitHub Actions guide](docs/github-actions.md) for
inputs, outputs, version selection, and pinning.

## Development

Install the current checkout when testing unpublished changes:

```sh
cargo install --path crates/rustscript
```

Run the repository checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The equivalence tests run the same examples through rustc and the interpreter
and compare their output byte for byte. The multifile conformance test does the
same for a deep module tree.

The differential harness generates deterministic, compile-valid Rust programs
and compares native and interpreted runs, including panics. Native is built
with overflow checks on, the debug default.

Its core is a type directed generator over the real type universe: all nine
integer widths, both floats, `bool`, `char`, `String`, `Vec<T>`, and
`Option<T>`. Generation is driven by types rather than by hand written cases,
so asking for a `u8` offers every literal, operator, cast, branch and bridged
method that can produce one, at any depth. Bridged methods live in one typed
catalog where a row states its receiver class, argument patterns and result
pattern, so a method added there immediately composes everywhere: inside a
condition, as the receiver of another call, or in a loop body. That matters
because a dimension the generator cannot name is a bug it cannot find, and the
older per-method case lists could only name `i64`, `bool` and `String`, which
is why a width bug in `saturating_add` survived every campaign that ran before
them.

Generated cases also cover ownership and borrowing, closures, structs, enums,
patterns, iterators, loops, `Result`, floats with their special values,
division, indexing, `unwrap`, and format specs. Numeric cases carry width
through annotated and suffixed bindings, inference, casts, compound assignment,
shifts, and negation across statements, the shapes a per-expression check
cannot see. Values the overflow lint would fold pass through an opaque helper,
so panics stay runtime events rather than compile errors. Some seeds splice
same-typed expression subtrees from other programs through replayable
structured mutation.

The generator covers what the language supports, never only what the
interpreter handles. Every divergence is a finding: the campaign prints
them after the run, grouped by bug with their seeds and saved artifacts,
and exits nonzero when there are any.

```sh
# Print one generated program.
cargo run -p rustscript-differential -- generate --seed 42

# Compare 10,000 programs and report every divergence, grouped by bug.
# The seed is random by default and printed at the start, pass --seed to
# replay a range.
cargo run --release -p rustscript-differential -- run --cases 10000
```

The campaign runs batches on all cores and exits nonzero when it finds a real
divergence, so a scheduled run can gate on it. Findings are grouped by
classification plus a short failure signature, so two different bugs with the
same classification stay apart. Unsupported-feature gaps do not fail the run,
but they are tracked too: the summary ranks them by reason with counts and
seeds, and one case per distinct reason is saved so it can be reproduced and
fixed. Saved cases live under `target/rustscript-differential/failures`; pass
`--stop-on-first` to halt and minimize the first finding. The minimizer holds both the classification and
the signature, so shrinking cannot drift to a different bug. The harness
batches native compilation, caches repeated reduction candidates, and stores
enough program data to replay every result.

The `Differential` workflow runs a campaign nightly on Linux, macOS, and
Windows. The base seed derives from the date and each OS adds its own offset,
so every night explores fresh disjoint seed ranges with nothing to track.
Saved finding and gap cases are uploaded as artifacts after every run, green
or red.

Minimized findings whose correct behavior is a panic are kept under
`crates/differential/regressions` and replayed by a test, since the
equivalence suite only covers examples that exit cleanly. `promote` routes a
fixed case to the regressions or to the examples automatically.

Every bridge and language feature must have an example under
`crates/examples/examples`. Examples build as real cargo binaries, and the
equivalence test runs each one compiled and interpreted, so every feature is
always tested against the real Rust compiler. A change the real compiler
cannot build has no coverage and is not done.

## Benchmarks

The benchmark suite compares RustScript with native Rust, Node, and Python on
equivalent programs. It records wall time, compute time, peak memory, raw
samples, and build provenance.

See the [benchmark guide](bench/README.md) for methodology and results, and the
[profiling guide](docs/profiling.md) for finding interpreter hot spots.

## Releases

RustScript is still 0.x, so minor versions may contain breaking changes. Exact
tags such as `v0.2.5` never move; the `v0.2` tag follows the newest patch in
that line. Pin an exact tag when a workflow must not change.

`rust update` installs the newest full release. It downloads the prebuilt
binary for the current platform, verifies it against the published
`SHA256SUMS`, runs it once to confirm the version, and only then replaces the
binary in the cargo bin directory. The previous binary is kept until the swap
succeeds, so a failed update rolls back. Cargo's own install list is updated
as well, so `cargo install --list` stays correct.

Prereleases and moving minor tags are never picked automatically. Naming a tag
installs exactly that version, so `rust update v0.2.3` also downgrades and
repairs a broken binary. `--from-source` builds with Cargo instead, which is
also the automatic fallback on a platform with no prebuilt binary.

## Licence

Dual licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
