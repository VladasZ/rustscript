# Roadmap

The performance goal is about the whole run, wall clock, process launch plus
startup plus work. That is what a script user experiences, and fast startup
is the interpreter's biggest structural advantage. The compute-only track is
a diagnostic, not the goal.

Two levels, judged per benchmark case against the interpreted rivals, Node
and Python.

- The floor. Never the slowest interpreted runtime on wall clock.
- The target. Faster than both rivals on wall clock.

Numbers are wall-clock medians from the committed
`bench/results/results.json`, one run on an Apple M1 Pro, 2026-08-18,
Node v26.7.0, Python 3.14.7. Re-run the suite and update this file when
the gaps change.

## Every case is at the target

All twenty-four cases beat both rivals on wall clock. The last four to get
there were the map and json cases, closed by the map table, span key, and
item probe extensions of the scalar for plans. How each case got there
lives in the git history and in [interpreter.md](interpreter.md), which
documents every specialization.

The tightest wins, the cases to watch when re-running:

| case      | rustscript | fastest rival | margin |
| --------- | ---------- | ------------- | ------ |
| nbody     | 63 ms      | node 64 ms    | 1.02x  |
| mandelbrot| 58 ms      | node 63 ms    | 1.09x  |
| collatz   | 55 ms      | node 62 ms    | 1.13x  |
| json_serialize | 76 ms | node 87 ms    | 1.14x  |
| binary_trees | 30 ms   | python 34 ms  | 1.15x  |

## Why the last four were losing

The pattern was one pattern. Python and Node win exactly where their hot
path leaves their interpreter. CPython runs `re`, `json`, string methods,
and dict internals as C code, and V8 JIT compiles hot loops to machine
code. RustScript executed the same loops as bytecode, one dispatch per
operation, with every value behind an `Arc`. The fix was the same fix:
translate those loops into scalar plans and run them unboxed, map probes,
span keys, and json item reads included.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
