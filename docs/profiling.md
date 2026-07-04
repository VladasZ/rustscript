# Profiling the interpreter

How to find out where the interpreter spends its time on macOS. The built-in
`sample` profiler is enough, no instrumentation and no special build, the
normal release binary works.

## Getting a useful sample

The bench cases in `bench/cases/` finish in 30-150 ms, which is too short for
`sample` to catch anything but process startup. Make a temporary variant of
the case that repeats the work about 10 times in a loop, or raise the input
size until the run takes 1-2 seconds. Keep these variants out of the repo.

```sh
cargo build --release -p rustscript
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

The full comparison against native Rust, Bun, and Python lives in `bench/`,
see `bench/README.md`. Quick run from `bench/`:

```sh
cargo run --release --bin bench -- --quick --no-gate
```

The bench invokes the `rust` binary from PATH, so install the current build
first with `cargo install --path crates/rustscript`, or the run measures a
stale binary.

## After a change

Run the test suite. The equivalence test compares interpreter output byte for
byte against the compiled examples, which must be built first. Rebuild them
after every bench run too, because the bench builds its native cases into the
same `target/release/examples/` directory and overwrites same named binaries
like `fib`:

```sh
cargo build --release -p rustscript-examples --examples
cargo test -p rustscript --release
```
