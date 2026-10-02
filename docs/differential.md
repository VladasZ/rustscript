# The differential harness

`crates/differential` generates random Rust programs, runs each one compiled
and interpreted, and compares the outputs. Any disagreement is a bug in the
interpreter, in the generator, or a gap the interpreter must declare. The
nightly `Differential` workflow runs it on Linux, macOS and Windows.

## How a case runs

The generator in `lang/` builds a typed program from a seed. Every node
carries its result type, so the generator, the renderer and the shrinker
agree without re-running inference. Every 4th seed is a structured mutation
of its predecessor, see `mutator.rs`. The runner compiles the program with
`rustc`, runs the same source through the interpreter, and classifies the
pair, see `Classification` in `runner.rs`. Matching output, a semantic
mismatch, a missing or spurious panic, a declared gap, a crash or a timeout.
The native binary runs twice, and a run where the 2 native runs disagree
means the grammar let nondeterminism through. It is counted and never
reported as a bug.

## Ownership and drops

A read of a non copy binding is a clone or a move, chosen by the generator
from the ownership state it keeps per binding in `lang/own.rs`. A moved
binding is gone until an assignment brings it back, a field moved out of a
struct or a tuple leaves the rest usable, and a move is offered only at the
loop and closure depth the binding was declared at. Nested bodies declare
their own `let`s, shadow outer names and drop them at the closing brace, a
bare `{ }` block does the same, and `std::mem::take`, `replace`, `swap`,
`Option::take`, `pop`, `remove` and `swap_remove` take values out in place.
A receiver a catalog method borrows is held while its arguments run, so no argument
writes its binding.

`DiffTrace` is a program local struct whose `Drop` prints its id. It sits in
locals, vec items, struct fields, option payloads, tuples, closure captures
and temporaries, so every move, scope end, loop iteration, `break`, `?` and
unwind becomes a line of output. Its `Clone` is derived and silent. It never
hashes, a hashed container would clone and drop it in an order real Rust
randomizes per process.

A shared borrow holds a binding without taking it. `for x in &v` holds `v` for
the loop, a reference an expression takes holds its binding to the end of the
statement, and a reference a `let` keeps holds it to the end of the scope. A
held binding is still read and borrowed again, it is never moved or written.
The right side of an assignment holds no reference, so the borrows it took are
over when the write happens, `v = v.as_slice().to_vec()`.
Only a `let` is written in place, a pattern binding or a parameter has no
`mut`. A closure that calls a captured `FnMut` closure is `FnMut` too, so its
`let` gets `mut`. A place scrutinee stays borrowed through every match guard,
so a guard never takes it by `&mut`. A comparison of non primitive values goes through
`PartialOrd`, so a place on its left stays borrowed while the right side runs. A
`+=` on an integer runs its right side first, `*m.entry(k).or_insert(0) += v`
included, so the checker reads `v` before the key.

## References

`&str` and `&[T]` are real types. A literal borrows nothing. `s.as_str()`, `&s[..2]`,
`v.as_slice()` and `&v[1..3]` borrow a `let`, or a temporary that ends with its statement.
Where a reference goes decides what it may borrow, see `RefRules` in `lang/own.rs`. One used
up inside its statement may borrow a temporary. One a `let` keeps holds its binding until
the scope of that `let` ends, and one that leaves an `if` branch, a match arm or a block
borrows nothing declared inside it. A binding that holds a reference is never written.

The catalog rows over `&str`, `&[T]`, `Option<&str>` and `Option<(&str, &str)>` are in
`catalog/rows_refs.rs`, and a `String` row takes its text arguments as `&str`. A whole
borrow has 2 more spellings, a bare `s` as the receiver of a call and `&s` as an argument.
Both are a reference only in that position, so the reducer and the mutator never move them.
A helper function takes `&str` and `&[T]` parameters, `match s.as_str()` has string
literal arms, and `if let Some(rest) = s.strip_prefix(..)` binds a reference.

## Loops over a collection

`for x in &v`, `for x in v.iter()` and `for (i, x) in v.iter().enumerate()` have a
statement body. The item stands behind a reference, so the body reads it through `(*x)`
and only clones it. A source that is a binding stays borrowed for the loop, any other source
is a temporary that drops right after it. `for (k, v) in &map` and `for x in &set` run in an
order real Rust randomizes, so the body only pushes one value per entry, a value that can
neither panic nor print a drop, and the statement sorts the vec after the loop. An
`iter_mut` loop writes each element with `*r = x`, `*r += x` or a method like `r.push(x)`.

## Mutable references

A `&mut` statement borrows one binding for its whole body, so nothing in it names that
binding. The forms are in `MutPlace`. The borrow block `{ let r = &mut x; .. }`, `&mut s.f0`,
`&mut v[i]`, `v.get_mut(i)`, `v.last_mut()`, `m.get_mut(&k)`,
`m.entry(k).or_insert_with(|| d)`, `for r in m.values_mut()` and
`if let Some(ref mut r) = opt`. The body clones the old value, writes through the reference
like an `iter_mut` loop does, and prints through it. A `values_mut` loop runs in a random
order, so its write sees its own entry alone, can neither panic nor print a drop, and prints
nothing.

## Closure parameters by reference

`filter` and `retain` hand their closure a reference. `RefParam` picks how the closure names
the item. A clone bound in the body, `|&x| ..` for a copy item, or `|x| ..` with the body
reading `(*x)`.

Inside a loop `v.extend(..)` never reads `v`. A vec that doubles in nested loops grows until
the native run times out.

## Binding forms

Patterns nest by type, `Some((a, 1u8..=5u8))`, and come with or patterns, `@`
bindings, `ref` bindings, const patterns and char ranges. A `ref` binding is
read through `(*name)` and only cloned. A `ref` binding into a place, `v` or
`v.0`, keeps that binding held for the whole arm or `if let` body, see
`Pat::pins`. The statements that bind through a
pattern are `if let` with let chains and an `else`, `while let Some(x) =
v.pop()` with the vec hidden from its body, `let else`, a `match` whose arms
are statement lists, and `let x = loop { .. break value; }`. `matches!` is an
expression. A nested body holds at most 2 more levels of these, so a program
stays finite. The reducer also tries each nested body alone in a bare block.

`own::check_block` replays the finished tree with the same rules. The
generator asserts it on every block it builds, the reducer drops every
candidate that fails it, and the mutator undoes a splice that fails it. The
rules are a subset of what `rustc` accepts, a scrutinee or a receiver read by
move counts as moved even where `rustc` would only borrow it.

## Commands

```text
cargo run --release -p rustscript-differential -- COMMAND

run [--seed N] [--cases N] [--timeout-ms N] [--stop-on-first]
surface [--refresh]
generate --seed N
mutate ARTIFACT --seed N
replay ARTIFACT
reduce ARTIFACT
promote ARTIFACT NAME
```

`run` drives a campaign. `generate` prints the program of one seed, so any
finding replays locally. `replay` re-runs a saved artifact, `reduce` shrinks
it to a minimal failing case, `mutate` grows a variant of it, and `promote`
copies the reduced case into `regressions/` under the given name. A
`RustcRejected` case keeps its first rustc error through the reduction, so it
never drifts to another program rustc rejects.

## Seeds

The nightly base seed derives from the date, so every night explores a fresh
disjoint range with no state to track. Each OS adds its own offset, so 3 jobs
cover 3 times the programs. Rerunning a night reproduces the same cases, and
a manual run takes any seed through `workflow_dispatch`. A seed replays only
with the generator that produced it, an old finding regenerates from the run
artifacts of its workflow, not from the seed.

## Artifacts

A failing case is saved under
`target/rustscript-differential/failures/seed-N-TIMESTAMP/` as `case.rs` plus
`artifact.json` with the classification and both outputs. A green run still
saves one case per distinct gap reason, and those are the input for closing
the gaps. The workflow uploads the whole directory on every run.

## Regressions

`regressions/` holds every promoted case. The `regressions` test replays each
one compiled and interpreted and requires full agreement, panic messages
included, so a fixed bug stays fixed. Fixing a new finding ends with a
`promote` of its reduced case.

## The surface report

`surface` compares the std surface against the method catalog and the
interpreter listing, so the methods neither side knows are in the log of
every run. `--refresh` re-harvests the std listing.

Rust leaves the sign of a computed NaN open, so it can differ between two
builds of the same program. The `is_sign_positive`, `is_sign_negative` and
`copysign` rows clear the sign of a NaN with `abs` before they read it.
`copysign` binds its 2 operands through an array, so an untyped float literal
argument takes the receiver type before `is_nan` reads it.
