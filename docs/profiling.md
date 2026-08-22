# Profiling the interpreter

How to find where the interpreter spends its time on `macOS`. The built in
`sample` profiler on the normal release binary is enough.

## Getting a useful sample

The bench cases in `bench/cases/` are too short for `sample` at default
sizes. Pass a size that makes the run take 1 to 2 seconds, for example
`sort/case.rs 2000000`. File driven cases need the fixtures generated first.

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

The "Sort by top of stack" section lists leaf functions by sample count.
Patterns seen so far:

- `_nanov2_free`, `nanov2_malloc_type`, `_malloc_zone_malloc` high means
  allocation bound. Look for per iteration `Value` allocations or clones.
- `vm::exec` and `vm_step::step` high with low malloc means the cost is
  opcode count, so fewer or fused ops help.
- `drop_in_place<Value>` and `Value as Clone::clone` mean value traffic. Look
  for clones that could be moves.
- `sip..Hasher` means something fell back to SipHash instead of the
  `FxBuildHasher` aliases in `value.rs`.

## Before timing anything

Check that the machine is quiet first. One busy app can make every case 3 to
4x slower, which looks exactly like a regression.

```sh
uptime                                     # load average should be well under
                                           # the core count
ps aux | sort -k3 -rn | head -5            # nothing unexpected above ~20% cpu
```

If something heavy is running, wait. Do not trust best of N under load.

## Timing

Every bench case prints `COMPUTE_NS` to stderr around the compute part. Run
it a few times and take the best:

```sh
./target/release/rust bench/cases/fib/case.rs 2>&1 >/dev/null | grep COMPUTE_NS
```

The full comparison against `rust`, `node` and `python` lives in `bench/`.
Quick run:

```sh
cargo run --release -p rustscript-bench --bin bench -- --quick
```

## After a change

Run the tests. The equivalence test compares interpreter output byte for byte
against the compiled examples.

```sh
cargo test --workspace
```
