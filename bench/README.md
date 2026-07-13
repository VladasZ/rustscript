# bench

Compares RustScript against native Rust, Node, and Python 3 on equivalent tasks.
The Rust file is both a compiled Cargo binary and the interpreted RustScript
source. TypeScript and Python use normal idioms for their runtimes. Every case
must print byte-identical stdout, and cases that write files must also produce
byte-identical files.

## Measurements

Each timed command runs as a fresh process. The harness records three tracks.

- Wall-clock time covers process launch, runtime startup, parsing, compilation,
  and work.
- Compute time comes from a timer inside each workload, after argument handling
  and immediately around the described work.
- Peak memory is the maximum resident set size reported for each run by
  `/usr/bin/time`.

The report and charts use the median for every track and retain every raw sample
in `results.json`. Timed stdout always goes to `/dev/null`, so wall and compute
runs see the same output destination. Samples run round-robin with a rotating
language order to spread temperature and background-system drift.

The default is three warmups, ten wall samples, and ten compute/memory samples
for every case and tier. `--quick` uses three samples per track. `--samples N`
sets both sample counts explicitly.

## Runtime behavior

Each runtime uses its default source-loading behavior. In particular, Node does
not receive an opt-in persistent compile cache. The harness builds and invokes
the workspace's `target/release/rust` directly rather than trusting a `rust`
binary from `PATH`.

Script validation is not part of `rust run`, so it is reported separately. The
suite records only an unchanged-script warm `rust check` cache hit. Priming and
measurement use an isolated temporary cache and never touch the user's
`~/.cache/rustscript`.

## Idiomatic tasks

The cases implement the same task and output, not mechanically identical
operations. Python uses `join` to build strings and a key function to sort.
JavaScript uses its normal regex splitting and collection methods. Container
representations and iterator allocation can differ between runtimes. This suite
measures programs a competent user would write, not equal VM instruction
streams.

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
- `sort`: idiomatic custom ordering.
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

Every compute case has a base tier and a 10x-work tier. The exact arguments and
fixture hashes are recorded in the report.

## Fixtures and provenance

`gendata` recreates all deterministic inputs and generated sources before every
run. Committed base fixtures stay synchronized with the generator. Large inputs,
temporary outputs, the HTTP server, and the check cache live under an isolated
temporary benchmark directory.

`results.json` records the Git commit and dirty state, RustScript binary hash,
benchmark-source hash, fixture hashes, tool versions, machine information,
settings, and all raw measurements. Charts include the commit and a visible
dirty-tree label when applicable.

## Running

The suite needs `node` and `python3` on `PATH`. Cargo builds all Rust binaries
it uses.

```
cargo run --release --bin bench
cargo run --release --bin chart
```

## Scope limits

The committed report is one run on one machine, not a cross-platform average.
The suite does not measure live internet services, CPU-parallel equivalents
across different concurrency models, cold dependency compilation, or cached
`rust build` mode. Results are directional evidence for the recorded machine
and versions, not a universal ranking of languages.
