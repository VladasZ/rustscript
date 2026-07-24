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

Runtime numerics match a default `cargo run`, which is debug Rust. Integer
overflow on `+`, `-`, `*`, `/`, and `%` panics instead of wrapping, and a
narrowing `as` cast truncates to the target type. This currently holds for
i64 and f64. Narrower widths are not tracked yet, so u8 through u32
arithmetic runs in i64, f32 runs as f64, and integer literals above i64::MAX
are rejected. These open gaps are listed in the differential quarantine
file, see Development below.

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
- `#[path]` module declarations and glob imports from script modules are not
  supported.
- `std::thread` is not supported; use Tokio tasks for parallel work.
- `static mut` is rejected. Plain statics behave like constants.
- Integer widths below 64 bits and `f32` carry no runtime meaning yet, values
  compute in i64 and f64.
- Lifetimes and generics are accepted but carry no runtime meaning.
- Serde container attributes such as `rename_all` and `default` are not yet
  implemented by the reflection bridge.

## Caching

Checks, compiled binaries, and shared Cargo dependencies live under
`~/.cache/rustscript`. Interpreted runs do not touch the cache. Entries unused
for 30 days are swept automatically on every check and build, and `rust clean`
removes everything at once.

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
with overflow checks on, the debug default. Generated cases cover typed
expressions, ownership and borrowing, collections, closures, structs, enums,
patterns, iterators, loops, `Result`, floats with their special values,
plain arithmetic that can overflow, division, indexing, `unwrap`, format
specs, and a catalog of bridged `String`, `Vec`, and map methods. Every
program also carries a numeric case that computes in real integer and float
widths, u8 through u64, usize, f32, and f64. Width flows through annotated
and suffixed bindings, inference, casts, compound assignment, shifts, and
negation across statements, the shapes a per-expression check cannot see.
Values the overflow lint would fold pass through an opaque helper, so panics
stay runtime events. Some seeds splice same-typed expression subtrees from
other programs through replayable structured mutation.

The generator covers what the language supports, never only what the
interpreter handles. Known divergences live in
`crates/differential/quarantine.toml`, keyed by classification and a
signature pattern where `*` matches any substring. The campaign prints them
with their notes but stays green, and a finding outside the list still fails
the run. Each entry is an open bug, fix the interpreter and delete the
entry.

```sh
# Print one generated program.
cargo run -p rustscript-differential -- generate --seed 42

# Compare 10,000 programs and report every divergence, grouped by bug.
cargo run --release -p rustscript-differential -- run \
  --seed 0 \
  --cases 10000 \
  --timeout-ms 5000
```

The campaign runs batches on all cores and exits nonzero when it finds a real
divergence, so a scheduled run can gate on it. Findings are grouped by
classification plus a short failure signature, so two different bugs with the
same classification stay apart, and unsupported-feature gaps are reported
separately without failing the run. Saved cases live under
`target/rustscript-differential/failures`; pass `--stop-on-first` to halt and
minimize the first finding. The minimizer holds both the classification and
the signature, so shrinking cannot drift to a different bug. The harness
batches native compilation, caches repeated reduction candidates, and stores
enough program data to replay every result.

The `Differential` workflow runs a campaign nightly on Linux, macOS, and
Windows. The base seed derives from the date and each OS adds its own offset,
so every night explores fresh disjoint seed ranges with nothing to track, and
failure artifacts are uploaded when a run finds something.

Minimized findings whose correct behavior is a panic are kept under
`crates/differential/corpus` and replayed by a test, since the equivalence
suite only covers examples that exit cleanly. `promote` routes a fixed case
to the corpus or to the examples automatically.

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
