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

All twenty-four cases beat both rivals on wall clock. The last case to
slip was binary_trees, where Python's C-level allocator had erased the
startup advantage, closed by extending function plans to recursive enums,
the binary-trees shape of building and folding a boxed tree. Before that
the map and json cases were closed by the map table, span key, and item
probe extensions of the scalar for plans. How each case got there lives
in the git history and in [interpreter.md](interpreter.md), which
documents every specialization.

The tightest wins, the cases to watch when re-running:

| case      | rustscript | fastest rival | margin |
| --------- | ---------- | ------------- | ------ |
| file_transform | 39 ms | python 41 ms  | 1.06x  |
| nbody     | 60 ms      | node 65 ms    | 1.07x  |
| mandelbrot| 58 ms      | node 65 ms    | 1.11x  |
| json_serialize | 77 ms | node 87 ms    | 1.12x  |
| collatz   | 54 ms      | node 63 ms    | 1.16x  |

## The recurring pattern

Every case that ever lost, lost the same way. Python and Node win exactly
where their hot path leaves their interpreter. CPython runs `re`, `json`,
string methods, dict internals, and object allocation as C code, and V8
JIT compiles hot loops to machine code. RustScript executed the same
work as bytecode, one dispatch per operation, with every value behind an
`Arc`. The fix is always the same fix: translate the hot shape into a
plan and run it unboxed, map probes, span keys, json item reads, and
recursive enum construction included.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
