# RustScript plans

Planned work on [RustScript](https://github.com/VladasZ/rustscript), the
interpreter at `~/dev/rustscript`. Each entry says what, why, and what it
depends on. Reproduced bugs live in [flaws.md](flaws.md),
not here.

## Declarative bridge registry

What. One `bridge!` declaration per receiver, rows of `name => handler`. The
build generates the `BuiltinId` enum, the dispatch match, and the `rust check`
coverage table from that one declaration. `method_names.txt`,
`path_names.txt`, the `BRIDGES` list in `bridge_tables_build.rs`, and the
string literal harvesting of the interpreter's own source go away.

Why. Today the coverage checker is built by parsing 100 hand listed functions
with `syn` and collecting every string literal inside them. A renamed function
panics the build, a stray string literal widens the checker, and a method the
checker vouches for is not by construction a method that dispatches. With one
declaration the two cannot drift.

Depends on. Nothing in the runtime. Deferred because it touches every bridge
file and would bury the ownership and typing rewrite in an unrelated diff.

## Single thread values

What. Move the VM from `Arc<Mutex<_>>` to `Rc<RefCell<_>>`. `tokio::spawn`
deep copies what the `async move` block captures into a `Send` form and runs
the task on its own VM thread. Only `Arc` and `Mutex` cells stay real shared
handles across tasks.

Why. Every field read takes a lock, `eq_value` clones both whole vectors on
every `==` to avoid locking the same mutex twice, and CI carries a deadlock
detector. None of that is needed for a single threaded script.

Depends on. The ownership rewrite being stable and measured first. Real moves
already make the locks uncontended, so this is a measured decision, not a
correctness one.

## Typed plan tier

What. A hot loop tier that runs a `while` or `for` body unboxed, built on the
inference table. The compiler already knows the scalar type of every register
in the body, so the tier translates once with no speculation and no fallback.

Why. The old plan tier guessed types at runtime and fell back on a miss, which
doubled every scalar op and left 12 files of speculation. It was deleted in the
ownership rewrite. Without it a 20M iteration `while` runs 3 to 4 times slower
than before, 1.75s against 0.45s, with a method call inside 2.7s against 0.6s.
The typed ops recovered about 20 percent, the rest is per op dispatch on a
boxed `Value`.

Depends on. Nothing, the inference table is in. Measure before and after on
a 20M iteration `while` that adds into an `i64`, and on the same loop with an
integer method call such as `wrapping_mul` in the body.

## Drop for temporaries

What. Emit a drop for a register that holds a `Drop` value and is not bound to
a name, where the liveness pass says it dies.

Why. `v.into_iter().next()` never drops the leftover items. Reproduced in
[flaws.md](flaws.md).

Depends on. Nothing, the liveness pass already has the death point.

## Partial move of a `Drop` field

What. A move of one field out of a struct that stays alive marks that field as
gone, so the struct's own drop skips it.

Why. Today the move copies and the field drops twice. Reproduced in
[flaws.md](flaws.md).

Depends on. A per struct moved field set in the frame, or a tombstone value in
the field slot.

## Compile time `From` for `?` and untyped `into()`

What. Give every bridge result a typed error in the inference table, so `?`
knows the source error type and picks the `From` impl when compiling. The same
table then types the last `into()` calls that still resolve at runtime.

Why. The ownership rewrite moved `T::from(x)` and typed `into()` to compile
time. `?` error conversion and `into()` on a value the pass could not type are
the last runtime lookups, they give the right result but keep the dynamic path
alive.

Depends on. Error types on every bridge signature, about two days across the
bridge files.
