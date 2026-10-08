# Robustness plan

Why scripts keep hitting bugs and gaps, and the plan to end that. Written on
2026-10-08 against release `0.6.58`. Nothing here is built yet. Single planned
features live in [plans.md](plans.md), open bugs in [roadmap.md](roadmap.md).

One thing is fixed and is not a question of this plan. RustScript is an
interpreter and stays one. It starts at once, and it runs on a machine with no
Rust toolchain. A plan that compiles the script with `rustc` is out.

## The problem

A script written as plain Rust often meets a missing method or a wrong result.
Each case is fixed fast, and the next one comes. The 40 commits from 2026-09-28
to 2026-10-08 show it:

- 16 release bumps.
- 8 commits add a missing std or crate method by hand.
- 8 commits fix a wrong result the generator found, mostly drops and integer
  widths.
- 5 roadmap notes and doc moves.
- 2 fixes of name lookup in modules.
- 1 fix of the update tool.

No commit removed a whole class of bug. Each one fixed single cases.

## Root causes

### Types and ownership are thrown away early

`lower_in` in `compile/infer/mod.rs` lowers `&T` to plain `T`. An array or a
slice becomes a `Vec`. So the type model cannot say whether a value is owned or
borrowed. Other code guesses that back:

- flags per function in `compile/fn_state.rs`, borrowed parameters, reference
  locals, drop exemptions, owning iterators,
- the liveness pass in `compile/liveness.rs`, which picks copy or move by
  whether a register is read again,
- `consumes_receiver` in `compile/method.rs`, a hand list of method names that
  take `self` by value, matched by name with no type.

Each of them handles its cases. Together they must agree on every path, and a
value that crosses a closure, an iterator and a collection finds the path where
they do not.

The type pass is also partial. A method that is not in its table is `Unknown`,
and the runtime goes by what the value says. `tests/untyped_calls.txt` lists 92
such calls in the examples alone.

The open roadmap bug is a direct result. "Borrowed" is 1 flag on a whole
collection, so a map with borrowed keys and owned values cannot be described.
Passing the flag through 1 more path repairs the example, not the rule.

### The std library is copied by hand

There are 685 method names in `method_names.txt` and 448 paths in
`path_names.txt`. One method is written in 4 places. For `dedup_by_key` they
are:

- the name in `method_names.txt`,
- a type row in `compile/infer/methods.rs`,
- a runtime copy in `higher_order.rs`, which repeats in a comment what std does
  and calls `run_user_drop` itself,
- a catalog row in the differential generator.

`binary_search` in `vecmap.rs` copies the inner steps of std so it lands on the
same index. Iterators are the project's own state machine in `iterator.rs`. A
`HashMap` and a `BTreeMap` are both 1 `IndexMap` with a `sorted` flag, see
`value/map_store.rs`.

std has several thousand methods. A copy made by hand has no end, and every
copy can differ from the real code in a corner.

### Drops are modelled case by case

`Value` has no Rust `Drop`. Every drop is an op the compiler emits.
[interpreter.md](interpreter.md) needs about 130 lines to list the cases, and
65 of the 179 promoted regressions are about drops.

### The tests grow 1 by 1 too

The differential harness is strong. It has typed generation, a compare against
native Rust, mutation, a reducer, drop traces, width probes and 3 systems. Its
limits sit where the interpreter is weak:

- A catalog row is 1 hand written template per method.
- The generator has no general reference type. A reference never sits in a
  struct field, a map key or a closure signature. The open roadmap bug is
  exactly there.
- The drop tracer has a derived, silent `Clone`. An extra clone is never seen.
- `is_unsupported` in `crates/differential/src/runner.rs` falls back to a
  search for the word `unsupported` in stderr. A script that prints that word
  counts as a known gap.
- stderr before a panic header and exit codes other than the panic code are not
  compared.
- Some regressions pin what Rust leaves open. The std docs let `binary_search`
  return any matching index, and the regression pins the index of 1 std
  version.

Two more findings:

- [flaws.md](flaws.md) was checked against `0.6.8`. Some entries may be fixed.
- `join!` cannot be fixed with `spawn`. Real `join!` runs its arms inside 1
  task, so an arm can borrow from the caller. It needs a real model of a paused
  future.

## What others do

Every serious second implementation of a language follows 2 rules.

They do not rewrite the standard library by hand.
[RustPython](https://github.com/RustPython/RustPython) copies the real Python
library and writes only a small core itself.
[Miri](https://github.com/rust-lang/miri), the interpreter of the Rust project,
runs the real std code.

They do not write their own tests for it. RustPython runs the test suite of
CPython and has a script that lists what is still missing, see
[its library update issue](https://github.com/RustPython/RustPython/issues/5104).
Miri runs the tests and the doc examples of core, alloc and std through
[miri-test-libstd](https://github.com/rust-lang/miri-test-libstd).

RustScript does the opposite on both today.

Other sources this plan uses:

- [rustdoc JSON](https://docs.rs/about/rustdoc-json) lists every method of a
  crate with its full signature.
- `rustc` builds drops with a scope stack, see
  [scope.rs](https://doc.rust-lang.org/nightly/nightly-rustc/src/rustc_mir_build/builder/scope.rs.html),
  and with drop flags, see
  [drop elaboration](https://rustc-dev-guide.rust-lang.org/mir/drop-elaboration.html).
- [SyRust](https://arxiv.org/abs/2104.12064v3) builds test programs for a Rust
  library from its type signatures.
- [Csmith](https://users.cs.utah.edu/~regehr/papers/pldi11-preprint.pdf)
  compares the output of generated programs.
  [Equivalence modulo inputs](https://www.microsoft.com/en-us/research/publication/compiler-validation-via-equivalence-modulo-inputs/)
  runs variants of 1 program that must behave the same.

## The plan

There are 4 parts. First the tests tell the truth, then the interpreter gets
the facts it guesses today, then it stops copying std, then the generator
follows. Every phase is judged by the number from phase 2.

### Part A, tests that tell the truth

**Phase 1, fix the test runner.**

- Only a structured `rust unsupported:` line counts as a known gap. The search
  for the word in stderr goes away.
- stdout, stderr and the exit code are compared in full.
- Each test is 1 of 3 kinds. Required by Rust, pinned to this std version, or
  open in Rust. `binary_search` among equal items is the third kind.
- Every entry of [flaws.md](flaws.md) is checked against the current version.

**Phase 2, borrow the tests that already exist.**

- Run the doc examples and the test files of std for the supported types. They
  carry their own asserts, so they need no compiler and run on any machine.
- Do the same for each bridged crate.
- Each test ends as pass, refused or wrong. A report lists what is missing,
  sorted by how often real scripts call it.
- This gives 1 number per release. The count of wrong must go to 0.

### Part B, give the interpreter the real facts

**Phase 3, one generated source for every method.**

- Read the real signatures of std and of the bridged crates from rustdoc JSON.
  This runs on a builder when RustScript itself is built, and the result ships
  as data.
- Generate from it the name table, the type row, whether the method takes
  `self` or `&self`, and the check table.
- This removes `method_names.txt`, the hand type rows, the name list in
  `consumes_receiver` and the harvest of string literals in
  `bridge_tables_build.rs`. It goes further than the bridge registry in
  [plans.md](plans.md), which declares the rows by hand.

**Phase 4, full types.**

- `&T`, `&mut T`, `Box`, `Rc`, slices and arrays stay distinct in the type
  model.
- Ownership is read from the type. "Borrowed" belongs to each value, never to a
  whole collection.
- Inside the supported subset an `Unknown` type is an error before the run,
  never a guess at runtime.

**Phase 5, one drop algorithm.**

- A scope stack with scheduled drops and drop flags, the way `rustc` does it,
  over the typed form of phase 4.
- The drop cases in [interpreter.md](interpreter.md) become 1 rule set.

### Part C, stop copying std

**Phase 6, real std code over `Value`.**

- `Value` gets real `Drop`, `Clone`, `Eq`, `Ord` and `Hash` that call the impls
  of the script.
- A script `Vec` is a real `Vec<Value>` and a `BTreeMap` a real
  `BTreeMap<Value, Value>`. An iterator is a real std adapter whose closure
  calls the VM.
- A script panic is a real Rust unwind, so std cleans up the way it really
  does.
- `dedup_by_key` becomes 1 line, with the right closure calls and drops by
  construction.
- It goes 1 receiver at a time. `Vec`, then maps and sets, iterators, `Option`
  and `Result`, strings, numbers. Each step deletes hand code.

**Phase 7, bridged crate types as real values.**

- A `PathBuf`, a `Regex` or a `DateTime` is held as the real type. The glue
  that calls its real methods is generated from phase 3.
- What the generator cannot express is printed as a list and written by hand.

### Part D, the generator and the rest

**Phase 8, a generator for the language only.**

- Method rows come from phase 3, not from hand templates.
- References are part of its types, so they appear in fields, keys and
  closures.
- Small combinations run in full. Ownership kind, container, operation, place
  of the type, and the way the scope ends.
- Each program also runs in equal spellings, a turbofish against a `let`
  annotation. That alone finds the open roadmap bug.
- The drop tracer gets a `Clone` that prints.

**Phase 9, real futures.** `join!` and `select!` poll their arms inside 1 task.
It depends on nothing above and comes last.

## Order and size

- Phases 1 and 2 come first. They are small, change no design, and give the
  number that judges everything after.
- Phase 3 unlocks phases 4, 6, 7 and 8. Phase 4 unlocks phase 5.
- Phases 4, 5 and 6 are the big ones, weeks each. This is a rough guess, not a
  measurement.

## The rule for daily work

A fix is done when the broken rule is named and a family of generated programs
covers it. One repaired example is not enough.

## Not known yet

- How many std doc examples the interpreter of today can load at all. Phase 2
  answers it.
- How far the current drop code is from 1 algorithm. It was not read in full
  for this plan.
- Whether real std iterators behind a boxed trait cost speed in a hot loop.
- Whether `Box`, `Rc`, `Arc` and `RefCell` are also lowered to their inner type
  in the type pass. `lower_in` was read, `lower_path` was not.
