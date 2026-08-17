# RustScript

[![Crates.io](https://img.shields.io/crates/v/run-rs.svg)](https://crates.io/crates/run-rs)
[![CI](https://github.com/VladasZ/rustscript/actions/workflows/ci.yml/badge.svg)](https://github.com/VladasZ/rustscript/actions/workflows/ci.yml)
[![Marketplace](https://img.shields.io/badge/marketplace-rustscript--action-2088FF?logo=githubactions&logoColor=white)](https://github.com/marketplace/actions/rustscript-action)
[![Licence](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue)](#licence)

[![Linux](https://img.shields.io/badge/linux-x86__64%20%7C%20arm64-informational?logo=linux&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)
[![macOS](https://img.shields.io/badge/macos-universal-informational?logo=apple&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)
[![Windows](https://img.shields.io/badge/windows-x86__64%20%7C%20arm64-informational?logo=windows&logoColor=white)](https://github.com/VladasZ/rustscript/releases/latest)

Write your helper scripts in Rust and run them like shell scripts, no compile
step. RustScript interprets a practical subset of the language, so a script
starts instantly. `rust check` validates the same file with the real `rustc`.

[docs/interpreter.md](docs/interpreter.md) explains how it works inside.

## Install

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

Make it executable and run it:

```sh
chmod +x notes.rs
./notes.rs
```

## Usage

```text
rust FILE.rs         interpret the script
rust -e 'CODE'       run a snippet, arguments after CODE go to it
rust check FILE.rs   validate without running
rust build FILE.rs   compile, cache, and run a native binary
rust supported       list every bridged method per receiver
rust clean           clear cached checks and builds
rust update [VER]    install a release, the newest one by default
rust --version       show version and build information
```

Arguments after the file go to the script.

A script run directly cannot use `rust build`, so the word `cmp` as the first
argument does the same thing. `./tool.rs cmp one two` compiles the script and
runs the binary with `one two`. Because of this a script must not treat its
own first argument as `cmp`, that word is reserved.

## What works

Functions, closures, structs, enums, patterns, loops, iterators, `Vec`,
strings, maps, sets, `Option`, `Result`, `?`, formatting, modules, local path
crates, and async with `#[tokio::main]`, spawned tasks, timers, and HTTP.
Traits with default methods, user `Display`, `Debug`, `Drop`, operator, and
`Iterator` impls, associated consts, `u128` and `i128`, `mem::swap` and its
siblings, and real sharing through `Rc`, `Arc`, `RefCell`, `Cell`, and
`Mutex`. Values copy on write, so clones and `Copy` assignments mutate
independently, exactly like compiled Rust.

The standard library bridge covers files, paths, stdin and stdout, processes,
TCP sockets, environment, time, and collections. Bridged crates include
[`anyhow`](https://github.com/dtolnay/anyhow), [`serde`](https://serde.rs),
[`serde_json`](https://github.com/serde-rs/json),
[`reqwest`](https://github.com/seanmonstar/reqwest),
[`regex`](https://github.com/rust-lang/regex), [`tokio`](https://tokio.rs),
[`chrono`](https://github.com/chronotope/chrono),
[`rand`](https://github.com/rust-random/rand), and more. Windows builds also
bridge [`winreg`](https://github.com/gentoo90/winreg-rs),
[`windows-service`](https://github.com/mullvad/windows-service-rs), and
[`wmi`](https://github.com/ohadravid/wmi-rs).

The full generated list of bridged methods is in
[docs/supported.md](docs/supported.md). Every feature has a working example
under `crates/examples/examples`.

## Limitations

- Crates without a bridge stop with an `unsupported crate` error.
- `std::thread` is not supported, use `tokio` tasks.
- `static mut` is rejected. Plain statics behave like constants.
- Lifetimes are accepted but mean nothing at runtime. Generic bounds
  dispatch by the value's runtime type.
- Glob imports from script modules are not supported.
- `HashMap` iterates in insertion order. Real Rust's order is arbitrary and
  unpromised, so a correct script cannot observe the difference, but an
  interpreted run is deterministic where a compiled one is not.

## Modules and local crates

`mod name;`, nested modules, and `crate::`, `self::`, `super::` imports work
as in normal Rust. A script inside a Cargo project can use local library
crates declared as path dependencies in the nearest `Cargo.toml`. See
[docs/multifile.md](docs/multifile.md) for layout rules and a complete
example.

## Caching

Checks, compiled binaries, and shared Cargo dependencies live under
`~/.cache/rustscript`. Entries are keyed by source hash and interpreter
version, unused ones are swept after 30 days. `rust clean` removes everything
at once.

## GitHub Actions

The repository is also a GitHub Action:

```yaml
- uses: VladasZ/rustscript@v0.2
  with:
    script: tools/release.rs
    args: --dry-run
```

It downloads a checksum verified prebuilt binary, so setup takes seconds
instead of compiling the crate. Linux, macOS, and Windows on x86_64 and
arm64. See [docs/github-actions.md](docs/github-actions.md).

## Benchmarks

RustScript is compared with native Rust, Node, and Python on equivalent
programs. See the [benchmark guide](bench/README.md) for methodology and
results, and the [profiling guide](docs/profiling.md) for finding interpreter
hot spots.

## Licence

Dual licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
