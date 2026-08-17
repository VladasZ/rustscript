# Roadmap

Work that is planned but not done. Each item says what is missing and, where
known, the direction the fix should take.

## Check gate

- Run the coverage walk before every interpreted run, not only in
  `rust check`, so an unchecked script cannot die on a cold branch after
  doing half its side effects. The walk is one linear pass over compiled
  bytecode. Needs a proper startup measurement with the `hello` and
  `big_script` bench cases before it lands.
- Per-receiver checking still falls back to the shared `BUILTIN_IDS` name
  list, so a bridged method name that exists on one receiver can vouch for
  another, for example a `Vec` name called on a `String`. Tightening this
  needs receiver tags on the id-dispatched surface.

## Performance

- `u64::from(x)` and the other numeric `T::from` calls compile as dynamic
  path calls, which costs a full dispatch with string hashing per call.
  Profiling `file_transform` showed this dominates its checksum loop.
  Lower them to the existing `Cast` op at compile time; rustc has already
  proven the conversion lossless, so the widening cast is equivalent.
- Iterator `for` loops pay a mutex lock and a `Step` indirection per
  element. A fast path in `for_next` for `Bytes`, `Chars`, and `Range`
  states would cut the per-iteration cost of tight loops.
- The committed bench results predate the copy-on-write value model and
  everything after it. Rerun the suite and recommit `bench/results`.

## Drop

Scope-end, explicit `drop(x)`, loop-iteration, `break`, `continue`, and
`return` all run user `Drop` impls in the right order. Still missing:

- `?` early returns leave the frame from inside the VM, skipping the scope
  drops the compiler would have emitted.
- Panic unwinding does not run drops.
- A guard passed by value into a function drops at the caller's scope end
  instead of the callee's, because the caller's register still holds a
  handle. Needs move-out at call sites for last uses.
- Guards inside containers, cells, or `Rc` are not dropped when their
  container dies.

## 128-bit integers

Literals, arithmetic, comparisons, casts, parsing, and formatting are real
at 128 bits. Still missing:

- The integer method surface (`checked_*`, `wrapping_*`, bit counts) runs
  through the shared i128 pipeline, so a `u128` receiver past `i128::MAX`
  gives wrong answers there. Route big receivers to native 128-bit method
  cores.
- Number format specs like `{:x}` on values past `i64::MAX` fall back to
  plain digits.
- A bare literal with a 128-bit annotation types only when it is the direct
  init. `let b: u128 = 1 << 100` still computes the shift in i64 and
  panics; `1u128 << 100` works. Annotation propagation into operand
  literals would fix it.

## Known approximations to document or fix

- `Rc::strong_count` subtracts the call's own two in-flight copies, but a
  borrow passed through further call hops still inflates the count by one
  per hop.
- A panicking task prints the real panic, but `handle.await`'s error
  payload debug-prints as a string, not as the real `JoinError` shape.
- `env::var`'s error is still a plain string, not a structured
  `VarError::NotPresent`.
- Errors from lazy line iterators (`reader.lines()`) are still plain
  strings, not structured io errors.
- `Mutex::new` treats every `Mutex` path as `std::sync::Mutex`;
  `tokio::sync::Mutex` needs its own async lock surface.
- A closure capturing a `let r = &mut v` alias from an enclosing function
  does not resolve the alias across the frame boundary.

## Release

The whole batch above ships as one minor release when the remaining items
land or are consciously deferred: commit, push, the Release workflow with
`bump=minor`, then the crates.io publish from the development machine.
