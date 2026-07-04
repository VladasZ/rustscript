# bench

Compares rustscript against native Rust, Bun, and Python 3 on the same programs.
Every case is one algorithm written three times, in Rust, TypeScript, and Python,
all printing byte identical stdout. The Rust file is both a real compiled cargo
example and a rustscript script, so one source feeds two of the four bars.

## What it measures

Two tracks, because they answer different questions.

- Wall-clock, via [hyperfine](https://github.com/sharkdp/hyperfine). Time from
  launch to exit, startup included. This is what you feel when you run a script.
- Compute only, self timed. Each case starts a clock right before the work and
  prints the elapsed nanoseconds to stderr as `COMPUTE_NS`. Startup is excluded,
  so native Rust shows its real compute speed instead of a startup floor.

rustscript is always measured warm, with the `cargo check` gate skipped through
`RUSTSCRIPT_SKIP_CHECK=1`. The gate is a one-time cost paid on the first run of a
new script, not a per-run cost, so it does not belong in a speed comparison
against Bun and Python. It is left out of the charts. For reference it runs a full
`cargo check`, around 5 seconds the first time a script changes, then every run is
warm at a few milliseconds. The bench still records `cold_mean` and `warm_mean` in
`results.json`.

## Cases

- `hello`, startup only, no compute timing
- `fib`, recursive fibonacci, call overhead
- `sieve`, sieve of eratosthenes, integer loops and vector indexing
- `mandelbrot`, float math in nested loops
- `collatz`, integer division and branching
- `word_count`, strings and a hashmap over a fixed text file
- `json`, parse a fixed json file and sum a field

The `word_count` and `json` inputs are committed under each case dir and are
produced by a seeded generator, so all languages read identical bytes. Regenerate
with `cargo run --bin gendata`.

## Running

```
cargo run --release --bin bench       full run, writes results/results.json
cargo run --release --bin chart       renders results/benchmark.png
```

Flags: `--quick` uses fewer samples, `--no-gate` reuses the previous gate number
so a rerun skips the slow cold rebuild.

Needs `hyperfine`, `bun`, and `python3` on PATH, and the `rust` binary installed
from `crates/rustscript`.

## The pictures

`chart` writes one PNG per case, `results/<case>.png`. Each compute case has two
panels, wall-clock and compute only. The startup case has just the wall-clock
panel. Bars carry a fixed color per language on a linear scale, with the exact
time printed on each bar.

Sizes were tuned once so warm rustscript takes a noticeable fraction of a second
on each compute case, then frozen, so runs stay comparable as the interpreter
gets faster. At those sizes native Rust finishes in well under a millisecond, so
its bar is a thin sliver next to rustscript. That is the honest picture, native
compute is effectively free at this scale, and the printed labels give the small
numbers the bars cannot show.
