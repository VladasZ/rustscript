# bench

Compares RustScript against native `rust`, `node` and `python` on the same
tasks. Python is pinned to `python3.14` by the `PYTHON` constant in
`src/lib.rs`, because stock `macOS` points `python3` at 3.9. The Rust file is
both the compiled binary and the interpreted source. Every case must print
byte identical stdout and files.

## Measurements

Each timed command is a fresh process. 3 tracks are recorded.

- Total time, from process launch to exit.
- Compute time, from a timer inside each workload around the work only.
- Peak memory, from `/usr/bin/time`.

Charts use the median and `results.json` keeps every raw sample. Stdout goes
to `/dev/null`. Samples run round robin with a rotating language order to
spread drift.

Default is 1 warmup and 5 samples per track. `--quick` uses 3. `--samples N`
sets it. `--case NAME` runs one case and replaces its entry in `results.json`.

## Runtime behavior

Each runtime uses its default source loading. Node gets no persistent
compile cache. The harness runs the workspace `target/release/rust`, not the
one on `PATH`.

`rust check` is reported separately as a warm cache hit, using an isolated
temporary cache.

## Idiomatic tasks

The cases implement the same task and output with the normal idioms of each
language. This measures programs a competent user would write, not equal VM
instruction streams.

## Cases

- `hello`: minimal process startup.
- `big_script`: startup with a generated thousand-line single file.
- `multifile_startup`: startup with roughly a thousand lines split across 30
  modules.
- `fib`: recursive calls.
- `sieve`: integer loops and indexed mutation.
- `mandelbrot`: nested floating-point loops.
- `collatz`: integer division and branching.
- `binary_trees`: allocation and recursive traversal.
- `string_builder`: string growth, search, and replacement.
- `higher_order`: idiomatic map, filter, fold, and predicate operations.
- `sort`: custom comparator ordering.
- `sort_key`: idiomatic cached or decorated key ordering.
- `hashmap_int`: integer-keyed map insertion and lookup.
- `nbody`: struct or record access and floating-point math.
- `json_serialize`: record construction and JSON serialization.
- `stdout_lines`: repeated use of each runtime's default print API.
- `word_count`: token counting and ranking over a fixed input.
- `json`: dynamic JSON parsing and field aggregation.
- `regex`: matching, captures, and replacement.
- `file_transform`: timed file read, line transformation, write, and re-read.
- `process_spawn`: repeated execution of the same benchmark-owned helper.
- `async_tasks`: task creation, cooperative scheduler yields, and joins using
  Tokio, promises, or `asyncio`, with no elapsed-time sleeps.
- `http_local`: persistent-client requests to a benchmark-owned loopback server
  with JSON responses.
- `automation`: a mixed config, file, regex, map, sort, and JSON-report script.

The exact arguments and fixture hashes are recorded in the report.

## Fixtures and provenance

`gendata` recreates all inputs and generated sources before every run.
Temporary outputs live in an isolated directory.

`results.json` records the commit, binary and fixture hashes, tool versions,
machine, settings and all raw measurements.

## Running

Needs `node` and `python3.14` on `PATH`.

```
cargo run --release --bin bench
cargo run --release --bin chart
```

`chart` writes one PNG per case into `results/` and rewrites
[RESULTS.md](RESULTS.md). The font is embedded from `fonts/`, so it renders
the same on every machine.

## Performance goal

Judged on total time including startup. The target is to beat both `node`
and `python` on every case. All cases currently do.

## Scope limits

The committed report is one run on one machine. It does not measure internet
services, parallel CPU work, cold dependency compilation or `rust build`
mode. I think of it as evidence for this machine, not a ranking of
languages.
