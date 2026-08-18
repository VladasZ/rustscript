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

Composite values are copy on write. Assignment and `clone` are refcount
bumps, and every mutable access site the compiler emits splits the value
from any sharing first, so a clone and its source mutate independently at
every depth while `&mut` borrows still write through to the borrowed place.
Sharing is observable only through the types real Rust shares with: `Rc`,
`Arc`, `RefCell`, `Cell`, and `Mutex` are real shared cells, and a write
through one handle shows through every handle. A compound assignment
through a lock guard, `*guard += n`, compiles to one fused op that holds
the cell's lock across the scalar read-modify-write, so parallel tasks
adding to a shared counter cannot lose updates.

Strings skip the mutex. A string is a shared `Arc<String>` buffer read
lock free, and `push` or `push_str` grows it in place when it is the only
handle, so a build up loop stays linear. A shared buffer is copied once on
the first append, which keeps value semantics.

Iterators are lazy and stateful, an iterator is a shared native resource like
a file handle. So `by_ref`, `peekable`, and open ended ranges keep their real
semantics instead of being faked with collected vectors.

Comparator sorts over plain ints specialize. When every element is an int
and the closure body compiles to int-only bytecode, `sort_by` translates
the body once into a small plan over flat `i64` registers in
`interpreter/scalar.rs` and sorts unboxed, skipping the closure call
machinery per comparison. Anything outside that subset, mutable captures
and arithmetic failures included, falls back to the generic path, so the
output never changes.

Scalar `for` loops specialize the same way. When the body compiles to
integer, float, and bool bytecode and the source is a string's `bytes()`,
an integer range, or a vec of scalars, `.iter()` and a pending `.skip(n)`
included, `interpreter/scalar_loop.rs` translates the body once into a
plan over unboxed values and `interpreter/scalar_for.rs` runs the whole
loop inside one dispatch, no `Value` per item and no per-op dispatch.
Arithmetic runs through the same width-checked cores, so overflow still
panics where debug Rust panics, and f64 math mirrors the generic float
paths, NaN comparison semantics included. Any runtime surprise rebuilds
the registers to the start of the failing iteration and hands that exact
item to the generic loop, so a fallback is invisible, panic line included.

Regex `find_iter` loops run as such plans too. Each match is an unboxed
span over the locked source, `m.start()` and `m.end()` read it directly,
and an integer `T::try_from(x)` whose value fits, plus the `.unwrap()` on
its result, run as plan ops mirroring the assoc conversion and the
`Result` method exactly. The chunks pull matches through the same span
walk the generic iterator uses, and a failing iteration rewinds the
offset so the generic loop re-pulls the identical match. A value out of
range, a `Span` used any other way, or a match past the u32 span bound
all fail the iteration over, so the `Err` value, the unwrap panic, and
the boxed match a later use needs come from the generic path unchanged.

Scalar `while` and `loop` loops specialize too, in
`interpreter/scalar_while.rs`. A `LoopHead` op sits right before every
loop head carrying the loop's backward jump, so the plan takes over at
loop entry and the first iteration already runs unboxed. The backward jump
that closes such a loop marks a region whose only exits are the jump back
to the head and the jumps to the op right after it, so the whole region,
condition included, runs as the same kind of plan inside one dispatch.
Pure numeric methods run inside plans through the same tables the generic
dispatch uses: the width-aware integer table for `is_multiple_of`, `min`,
`max`, `clamp`, the `saturating` and `wrapping` families and the bit
counts, and the f64 table for `sqrt`, rounding, and the sign and NaN
tests, picked by the receiver at run time. `as` casts between integer
widths and f64 and unshadowed `f64::from` calls run in plans too. A loop
that does not qualify records that in one atomic flag per op, so its
backward jump costs one load per iteration and nothing else.

While plans also run vec indexing, the sieve shape of `v[i]` reads and
`v[i] = x` writes. The region's vec bases resolve once at loop entry, each
written base splits from sharing the way the generic `UniqueReg` would,
and their storage stays locked while the plan runs, dropping around Ctrl-C
polls. Writes land immediately and go into a journal, and the registers
snapshot at every iteration boundary, so a failing iteration restores both
to its exact entry state and the generic loop re-runs it, out-of-bounds
panic line included. A non-vec base or two bases sharing one storage fail
over to the generic path unchanged.

Struct elements inside those vecs run unboxed too, the nbody shape of
`bodies[i].x` reads and `bodies[i].vx -= e` writes. An index whose value
the region reads fields through becomes an element handle holding the
struct's arc, field reads and journaled field writes go through it, and a
written element splits from sharing on its first write, once per run,
which leaves the same observable state as the generic per-access split. A
build-time check proves every handle use is preceded by its index in the
same iteration, so a failing iteration still re-runs generically with
identical state. A non-scalar field or element fails the iteration over
the same way.

Self-recursive scalar functions specialize as whole bodies, in
`interpreter/scalar_fn.rs`. When a function's recursion targets only
itself and every op of its body fits the plan subset, the fib shape, a
direct call runs the entire call tree unboxed inside one `CallFn`
dispatch, on a flat frame stack with no boxed values and no generic frame
machinery per call. The subset makes such a body pure, so recovery needs
no journal: any surprise, an overflow, a non-scalar argument, discards
the run and the generic path runs the whole call from scratch, panic op
and line included. The plan caps its depth exactly where the generic
loop's call depth cap sits, polls Ctrl-C mid-run, and a function that
does not qualify records that in one atomic flag, so its calls pay one
load each.

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

`u128` and `i128` are real: their values live in dedicated 128-bit storage,
arithmetic checks overflow natively at 128 bits, and casts, comparisons,
parsing, and formatting keep the full range. The method surface runs in
native 128-bit cores too, so `checked_*`, `wrapping_*`, the bit counts, and
the byte views answer correctly past `i64::MAX`, and the radix format specs
like `{:x}` print the full two's complement image. An integer annotation
reaches into its init's arithmetic, so `let b: u128 = 1 << 100` computes at
128 bits.

## Traits

Trait impls register their methods per concrete type, and default method
bodies fill in for the methods an impl does not override, so dynamic
dispatch picks the override where one exists. User `Display` and `Debug`
impls drive `{}`, `{:?}`, `to_string`, and `format!`. Operator trait impls
drive the operators, a user `Iterator` drives for loops and adaptor chains,
and associated consts resolve as `Type::NAME` globals.

`Drop` impls run where real Rust runs them: at scope end in reverse
declaration order, on explicit `drop`, per loop iteration, on `break`,
`continue`, `return`, and `?` early returns, and during panic unwinding for
every live local of every frame, innermost first. A guard passed by value
into a call drops at the callee's end, and guards inside containers, cells,
and `Rc` drop when their container dies. A value whose storage still has
another holder was moved or is still shared, so its real owner drops it,
which is also why a shared `Rc` cycle leaks exactly like real Rust's.

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
anything. Where the receiver's type is knowable, the method is checked
against that receiver's own surface, and the fast-dispatch method ids carry
receiver tags, so a `Vec` name called on a `String` is caught. Known gap,
only method calls are checked, not path calls like `std::process::exit`.

The same walk runs before every interpreted run, not only in `rust check`,
so an unchecked script cannot die on a cold branch after doing half its
side effects. It is one linear pass over the compiled bytecode and costs
less than measurement noise even on thousand-line scripts.
