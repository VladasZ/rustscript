# How the interpreter works

What happens between `rust FILE.rs` and the first line of output. Cargo and
`rustc` are not involved. The source is parsed, compiled to bytecode in
memory, and executed by a register VM. That whole pipeline takes milliseconds,
which is the reason a script starts instantly.

## The pipeline

Four steps, all in `crates/rustscript/src`.

1. The loader in `loader.rs` collects the source files. It follows `mod`
   declarations with the same directory rules as `rustc`, so `mod name;`
   loads `name.rs` or `name/mod.rs` at any depth. If the script lives inside
   a Cargo project, local path crates from the nearest `Cargo.toml` are
   grafted in as modules, so `use shared::x` resolves without a `mod`
   declaration.
2. Every file is parsed with [`syn`](https://github.com/dtolnay/syn).
3. The resolver gives every top level item a canonical key like `foo::bar`
   and resolves paths against the module they appear in. Imports, renames,
   `crate::`, `self::`, `super::`, and re-export chains are all handled here,
   at load time, not while the script runs.
4. The compiler in `interpreter/compile` lowers the AST into register
   bytecode. This runs once per program. Every variable becomes a numbered
   register slot, control flow becomes jumps, patterns become test and bind
   ops, and common macros like `println!` are lowered inline. Types are
   lowered into a plain IR too, so the compiled chunks carry no `syn` AST at
   all.

The result is a `Chunk` per function. The VM in `interpreter/vm.rs` executes
chunks and never touches the parse tree again. Variable access is an array
read by register number, not a name lookup, which is where most of the speed
comes from.

## The runtime

Values are backed by `Arc` with a `parking_lot` mutex for mutation, so every
value is `Send + Sync` and can move between threads.
The VM runs over a multi thread `tokio` runtime. A `#[tokio::main]` script
gets real concurrency, `tokio::spawn` puts a task on another worker thread,
and `.await`, `join!`, timers, and async HTTP behave like they do in compiled
Rust.

The interpreter ignores ownership at runtime. Everything is shared and
interior mutable, a `Vec` is an `Arc<Mutex<Vec<Value>>>`. That is safe
because `rustc` has already proven the program obeys the borrow rules, the
interpreter only needs to produce the same observable behavior.

Strings skip the mutex. A string is a shared `Arc<String>` buffer read
lock free, and `push` or `push_str` grows it in place when it is the only
handle, so a build up loop stays linear. A shared buffer is copied once on
the first append, which keeps value semantics.

Iterators are lazy and stateful, an iterator is a shared native resource like
a file handle. So `by_ref`, `peekable`, and open ended ranges keep their real
semantics instead of being faked with collected vectors.

## No second type system

The interpreter does not type check. `rustc` stays responsible for type,
ownership, borrowing, and visibility errors, that is what `rust check` runs
it for. The interpreter will never implement a second Rust type system.
Lifetimes and generics are accepted and mean nothing at runtime.

## Numerics match debug Rust

The target is simple to state, an interpreted run must be byte for byte equal
to a default `cargo run`, which is debug Rust. Panics included.

Values carry their real integer width at runtime, `u8` through `u64`,
`usize`, `i8` through `i64`, `f32`, and `f64`. A width tagged value lives in
one `i64`, and `u64` and `usize` keep their full range past `i64::MAX`.
Overflow on arithmetic, shifts, and negation panics exactly where debug Rust
panics. A narrowing `as` cast truncates, a float to integer cast saturates
with NaN going to zero, and `f32` computes and prints at `f32` precision.

Integer methods answer in the receiver's own width. `saturating_add` stops at
that width's bound, `wrapping_add` wraps at it, `checked_add` reports
overflow against it, and the bit methods count over the real width. A
function's declared numeric types act as widths too, the parameter retags
what the caller passed and the return type retags what the body produced.

Some result types are chosen by the caller, not the receiver, and the
interpreter honors what the source states. `parse` takes its target from the
turbofish, so `"300".parse::<u8>()` is an `Err`. `sum` takes its element type
the same way. `unwrap_or_default` builds its default from whatever annotation
the call site provides, a `None::<T>`, a binding annotation, or a `collect`
turbofish earlier in the chain.

`u128` and `i128` are the known gap, they compute in `i64`.

## Panics and errors

A panic prints the standard panic header with the failing file and line, then
a script backtrace, and the process exits with 101. An `Err` out of `main`
prints `Error: ...` and exits with 1. Same as compiled Rust in both cases.

## Crate bridges

Dependencies are never compiled. A call like `reqwest::get` or
`Regex::new` is dispatched to a native bridge, a Rust function inside the
interpreter that calls the real crate and wraps the result as a value. The
bridged crates are compiled into the `rust` binary itself.

Method cores are written once against plain Rust types and shared by
dispatch, so a method cannot drift between call forms. A crate without a
bridge stops the script with an `unsupported crate` error instead of
guessing.

## What `rust check` adds

`cargo check` proves the script is valid Rust. It cannot prove the
interpreter implements everything the script calls, because `"x".repeat(3)`
is valid Rust whether or not the bridge has `repeat`. And running the script
only proves it for the lines that actually execute.

So the coverage pass in `interpreter/coverage.rs` walks the compiled
bytecode instead. Every method call the VM could ever make is a single op
with a name, so every one is visible on every branch without executing
anything. Known gap, only method calls are checked, not path calls like
`std::process::exit`.
