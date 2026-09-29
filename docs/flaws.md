# RustScript flaws

Known broken or surprising behaviour in [RustScript](https://github.com/VladasZ/rustscript),
the interpreter that runs the `.rs` scripts in this repo. Every entry here was
reproduced by hand, and each one carries the exact steps so it can be rechecked
after an interpreter release.

This is not the same as a missing feature. A missing method stops the script with
a clear `rust unsupported` message, and the rule in the `rust` skill applies: add
the feature to the interpreter at `~/dev/rustscript`. The entries below are worse
than that. They run, and they are wrong or silent.

Related docs. [rustscript-port.md](https://github.com/VladasZ/thing-private/blob/main/docs/rustscript-port.md)
in the thing repo records the features the port of its scripts needed. The
interpreter's own `README.md` has a Limitations
section for gaps that are there by design, such as `std::thread` being refused in
favour of tokio tasks, so those are not repeated here.

Verified against `rustscript 0.6.8 (a17a006, built 2026-08-19)`.

## `filter` over `Vec<u8>` bytes removes nothing

An `into_iter().filter(...).collect()` chain over a `Vec<u8>` runs and
returns every byte, the predicate is never applied. Real Rust drops the
filtered bytes.

```rust
#!/usr/bin/env rust

fn main() {
    let bytes: Vec<u8> = vec![65, 0, 66, 0];
    let kept: Vec<u8> = bytes.into_iter().filter(|b| *b != 0).collect();
    println!("{}", kept.len());
}
```

- `rustc` prints `2`.
- `rust` prints `4`.

Hit for real in `shell/win/wsl.rs`, which stripped the zero bytes out of
UTF-16 `wsl.exe` output this way and got the raw bytes back, so the distro
check saw an empty string. Until it is fixed, avoid byte filtering, or avoid
needing it, the wsl stage now probes the distro with an exit code instead of
parsing output.

## A `const` as a match pattern never matches

A named constant used as a pattern inside `match` compiles, runs, and simply
never matches, so the arm is dead and the value falls through to `_`. Real
Rust treats a const pattern as an equality test.

```rust
#!/usr/bin/env rust

const REBOOT_EXIT: i32 = 3;

fn main() {
    let code = Some(3);
    match code {
        Some(0) => println!("ok"),
        Some(REBOOT_EXIT) => println!("reboot"),
        _ => println!("failed"),
    }
}
```

- `rustc` prints `reboot`.
- `rust` prints `failed`.

Hit for real in `shell/win/setup-run.rs`, where `Some(REBOOT_EXIT)` was meant
to catch the wsl stage asking for a reboot and the run reported FAILED
instead. Until it is fixed, compare with `==` or `if let` instead of naming a
const in a pattern.

## `join!` awaits in order and overlaps nothing

`tokio::join!` compiles, runs, and returns correct values, so nothing looks wrong.
It is simply not concurrent. Real Rust polls the arms together. This interpreter
finishes each one before starting the next, so a script that uses `join!` for
parallel requests is fully sequential.

Measured with three identical HTTPS requests in one script:

- `join!` of three, 1354 ms
- three plain sequential awaits, 1412 ms
- three `tokio::spawn` handles awaited in turn, 554 ms

Root cause is `compile_join_macro` in
`crates/rustscript/src/interpreter/compile/macros.rs`. It compiles each argument
with `compile_expr`, which runs an async call to completion right there, then
emits one `Await` per arm against a value that has already finished. No `Spawn`
op is ever emitted. Its own comment claims the opposite, that every task is
running before the first await. Only `tokio::spawn` reaches `compile_spawn`,
which is why spawn overlaps and `join!` does not.

Work around it with one `tokio::spawn` per request, which is what `myip.rs` in
this repo already does:

```rust
let a = tokio::spawn(async move { fetch(url_a).await });
let b = tokio::spawn(async move { fetch(url_b).await });
let ra = a.await??;
let rb = b.await??;
```

A fix would give `compile_spawn` an expression variant and have
`compile_join_macro` emit a `Spawn` per arm, then await the handles as it already
does. It needs an example under `crates/examples/examples` so the equivalence
suite covers it, written so stdout stays the same byte for byte between the
compiled and interpreted runs. That means asserting a duration is under a
threshold rather than printing the duration.

## An unresolved crate is a check error but a silent `None` at runtime

`rust check` and `rust` disagree, and the run is the one that lies.

Reproduce with any script that imports a crate it cannot see, for example a
script outside this workspace that does `use shared::http;`:

```rust
#!/usr/bin/env rust

use shared::http;

fn main() {
    let r = http::get("https://example.com", &[("Accept", "application/json")]);
    println!("debug={r:?}");
}
```

- `rust check script.rs` correctly fails with `E0432`, unresolved import
  `shared`, and suggests `cargo add shared`.
- `rust script.rs` runs, and `r` prints as `None`. It is not `Ok`, it is not
  `Err`, so a `match` over both arms panics with `no match arm matched the value`.

The same file inside the workspace prints `status=200`. So the trap is that a
script's behaviour depends on where the file sits, and the failure surfaces as a
nonsense value far from its cause. Copying a script to `/tmp` to try something out
is exactly how you meet this.

Until it is fixed, run a test script from inside the workspace whose crates it
imports, and run `rust check` on it before trusting anything it printed.

## `tokio::spawn` only accepts a literal async block

Real Rust takes any future. Here the argument has to be an `async` block written
at the call site.

```rust
tokio::spawn(fetch(url));                          // rust error: tokio::spawn needs an async block
tokio::spawn(async move { fetch(url).await });     // fine
```

The error is clear and it happens at compile time, so this one only costs a
rewrite. It is listed because the rewrite forces owned values into the block,
which is why call sites end up cloning strings before spawning.

## `tokio::task::spawn_blocking` is missing

```rust
use tokio::task::spawn_blocking;
let h = spawn_blocking(probe);   // unknown method `spawn_blocking` on closure
```

This matters because it is the normal bridge for running blocking work under a
runtime. Without it there is no correct way to call a blocking API from a
`#[tokio::main]` script, which leads directly into the next entry.

## `await` is rejected anywhere in a program without `#[tokio::main]`

The check is per program, not per function, and it reaches into the crates the
script imports. Adding a single `async fn` to a shared library crate breaks every
synchronous script that imports that crate, with an error pointing at the entry
point rather than at the library:

```
rust error: `.await` is only available under #[tokio::main]
```

This was hit for real. Async twins of `shared::http::get` were added to
`shell/rs/shared/src/http.rs` so a `#[tokio::main]` script could run two requests
at once. `us.rs`, a plain synchronous script that imports the same crate, stopped
running. The change was reverted.

The consequence is a rule, not a workaround. A `#[tokio::main]` script keeps its
own async client rather than putting one in `shared`. Both `myip.rs` and the poker
skill helper do this, and the module comment at the top of
`shell/rs/src/bin/myip_parts/net.rs` records the same conclusion for its host
helpers.

## `rust check` does not catch any of the above

Every program above passes `rust check`, because `cargo check` proves they are valid
Rust and the coverage walk only checks that every called method exists in the
bridge. Neither gate sees the runtime semantics. So a green check and a green
run together still do not prove the output matches compiled Rust. Compile and
run the real binary once, `rust build FILE.rs`, when the script's result
matters.
