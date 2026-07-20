# Profiling the interpreter

How to find out where the interpreter spends its time on macOS. The built-in
`sample` profiler is enough, no instrumentation and no special build, the
normal release binary works.

## Getting a useful sample

The bench cases in `bench/cases/` finish quickly at their default sizes, which
is too short for `sample` to catch anything but process startup. Most sized
cases take the size as their single argument, so pass one big enough that the
run takes 1-2 seconds, for example `sort/case.rs 2000000`. Generate the large
file fixtures into a temporary directory when profiling file-driven cases.

```sh
cargo run --release -p rustscript-bench --bin gendata -- /tmp/rustscript-bench-fixtures
```

```sh
cargo build --release -p run-rs
export RUSTSCRIPT_SKIP_CHECK=1

./target/release/rust /tmp/case_prof.rs >/dev/null &
PID=$!
sleep 0.05                       # skip parse and compile startup
sample $PID 2 -f /tmp/prof.txt   # sample for 2 seconds at 1 ms
wait $PID

sed -n '/Sort by top of stack/,/Binary/p' /tmp/prof.txt
```

## Reading the output

The "Sort by top of stack" section lists leaf functions with the number of
samples that landed in each. Patterns that have come up so far:

- `_nanov2_free`, `nanov2_malloc_type`, `_malloc_zone_malloc` high means the
  workload is allocation bound. Look for per-iteration `Value` allocations,
  string clones, or temporary containers.
- `vm::exec` is the dispatch loop itself. A high share here with low malloc
  means the remaining cost is opcode count, so fewer or fused instructions
  help.
- `drop_in_place<Value>` and `Value as Clone::clone` mean register and value
  traffic. Look for clones that could be moves.
- `sip..Hasher` means something fell back to the default SipHash hasher
  instead of the `FxBuildHasher` aliases in `value.rs`.

## Before timing anything

Check that the machine is quiet first, or the numbers are garbage. A game, a
video call, or one busy app can make every case read 3-4x slower with huge
run-to-run variance, which looks exactly like a code regression.

```sh
uptime                                     # load average should be well under
                                           # the core count
ps aux | sort -k3 -rn | head -5            # nothing unexpected above ~20% cpu
```

If something heavy is running, ask to close it and wait. Do not benchmark
around it and do not trust best-of-N under load.

## Timing

Do not take wall-clock numbers from the profiler. Every bench case prints
`COMPUTE_NS` to stderr around the compute part only. Run the case a few times
and take the best:

```sh
./target/release/rust bench/cases/fib/case.rs 2>&1 >/dev/null | grep COMPUTE_NS
```

The full comparison against native Rust, Node, and Python lives in `bench/`,
see `bench/README.md`. Quick run from the workspace root:

```sh
cargo run --release -p rustscript-bench --bin bench -- --quick
```

The harness builds and invokes the workspace's `target/release/rust` directly.

## After a change

Run the test suite. The equivalence test compares interpreter output byte for
byte against the compiled examples. Benchmark workloads are separate Cargo
binary targets, so they do not share the examples output directory.

```sh
cargo test --workspace
```
