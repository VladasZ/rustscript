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

`DiffTrace` is a program local struct whose `Drop` prints its id. It sits in
locals, vec items, struct fields, option payloads, tuples, closure captures
and temporaries, so every move, scope end, loop iteration, `break`, `?` and
unwind becomes a line of output. Its `Clone` is derived and silent. It never
hashes, a hashed container would clone and drop it in an order real Rust
randomizes per process.

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
copies the reduced case into `regressions/` under the given name.

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
