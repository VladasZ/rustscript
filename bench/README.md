# bench

Compares rustscript against native Rust, Node, and Python 3 on the same programs.
Every case is one algorithm written three times, in Rust, TypeScript, and Python,
all printing byte identical stdout. The Rust file is both a real compiled cargo
example and a rustscript script, so one source feeds two of the four bars.

## What it measures

Three tracks, because they answer different questions.

- Wall-clock, via [hyperfine](https://github.com/sharkdp/hyperfine). Time from
  launch to exit, startup included. This is what you feel when you run a script.
- Compute only, self timed. Each case starts a clock right before the work and
  prints the elapsed nanoseconds to stderr as `COMPUTE_NS`. Startup is excluded,
  so native Rust shows its real compute speed instead of a startup floor.
- Peak memory, max RSS via `/usr/bin/time`, taken from the same runs as the
  compute samples.

Compute samples are interleaved round robin across the four languages, so slow
thermal drift spreads evenly instead of biasing whichever language runs last.
`results.json` records the tool versions, hardware, and date of the run, so old
and new numbers stay comparable.

## Two size tiers

Every compute case runs twice, at a base size and at 10x. At the base sizes a
Node run is mostly startup and JIT warmup, which flatters ahead-of-time
runtimes on wall clock. The big tier shows the other regime, where the work
dominates and V8 has warmed up. Both pictures are honest, they answer different
questions: "run a small script once" versus "chew through real work".

## Caching, both sides

rustscript is always measured warm, with the `cargo check` gate skipped through
`RUSTSCRIPT_SKIP_CHECK=1`. The gate is a one-time cost paid on the first run of a
new script, not a per-run cost, so it does not belong in a speed comparison
against Node and Python. For reference it runs a full `cargo check`, around 5
seconds the first time a script changes, then every run is warm at a few
milliseconds. The bench still records `cold_mean` and `warm_mean` in
`results.json`.

For symmetry Node gets `NODE_COMPILE_CACHE`, its own on-disk V8 compile cache,
so it also does not pay type stripping and compilation on every measured run.

## Implementation standard

Each case is written the way a competent user of that language would write it,
no micro tuning, no waste. Where languages differ in idiom the natural idiom
wins: Python builds strings with `join`, sorts with a key function instead of a
comparator, and JS splits on a regex. The `stdout_lines` case deliberately
keeps each language's default print and buffering, that policy is part of what
it measures.

## Cases

- `hello`, startup only, no compute timing
- `big_script`, startup on a generated thousand line source, parse and compile
  scaling
- `fib`, recursive fibonacci, call overhead
- `sieve`, sieve of eratosthenes, integer loops and vector indexing
- `mandelbrot`, float math in nested loops
- `collatz`, integer division and branching
- `binary_trees`, allocate and drop millions of small nodes, allocation churn
- `string_builder`, grow a large string, then search and replace in it
- `higher_order`, map, filter, fold, any with closures over a vector
- `sort`, sort through a per element callback
- `hashmap_int`, integer keyed hashmap insert and lookup
- `nbody`, struct field access and float math, the classic 5 body simulation
- `json_serialize`, build records and stringify them
- `stdout_lines`, print tens of thousands of lines, default buffering
- `word_count`, strings and a hashmap over a fixed text file
- `json`, parse a fixed json file into dynamic values and sum fields
- `regex`, match, capture, and replace over the word_count corpus

The `word_count` and `json` inputs are committed under each case dir and are
produced by a seeded generator, so all languages read identical bytes. The 10x
`data_big.*` inputs are too large for git, they are gitignored and `bench`
regenerates them on demand. The `big_script` sources are also generated but
committed, since the Rust one must exist as a cargo example. Regenerate all
with `cargo run --bin gendata`, the big inputs with `cargo run --bin gendata
-- --big`. Case sizes are passed as a single argument with the base size as the
default, so any case runs standalone too.

## Running

```
cargo run --release --bin bench       full run, writes results/results.json
cargo run --release --bin chart       renders one PNG per case and tier
```

Flags: `--quick` uses fewer samples, `--no-gate` reuses the previous gate number
so a rerun skips the slow cold rebuild.

Needs `hyperfine`, `node`, and `python3` on PATH, and the `rust` binary installed
from `crates/rustscript`.

## The pictures

`chart` writes one PNG per case and tier, `results/<case>.png` and
`results/<case>_big.png`. Compute cases get three panels, wall-clock, compute
only, and peak memory. The startup cases skip the compute panel. Bars carry a
fixed color per language on a linear scale, with the exact value printed on
each bar. The two time panels share one axis, the memory panel has its own.

Base sizes were tuned once so warm rustscript takes tens of milliseconds on
each compute case, then frozen, so runs stay comparable as the interpreter gets
faster. At those sizes native Rust finishes in well under a millisecond, so its
bar is a thin sliver next to rustscript. That is the honest picture, native
compute is effectively free at this scale, and the printed labels give the
small numbers the bars cannot show.
