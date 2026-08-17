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
`bench/results/results.json`, one run on an Apple M1 Pro, 2026-08-17, with
the scalar loop specialization included, Node v26.7.0, Python 3.14.7. Re-run
the suite and update this file when the gaps change.

## Below the floor

Nine cases where RustScript is the slowest of the three, worst first. Gap is
the RustScript median divided by the slowest rival's median. The compute gap
column shows the same ratio on the compute-only track, which tells how much
of the loss is raw interpreter speed rather than startup.

| case        | wall gap | rustscript | slowest rival | compute gap |
| ----------- | -------- | ---------- | ------------- | ----------- |
| collatz     | 2.1x     | 189 ms     | python 88 ms  | 3.0x        |
| regex       | 1.7x     | 178 ms     | python 102 ms | 2.2x        |
| nbody       | 1.6x     | 230 ms     | python 141 ms | 2.0x        |
| fib         | 1.6x     | 98 ms      | node 63 ms    | 3.6x        |
| automation  | 1.4x     | 146 ms     | python 103 ms | 1.8x        |
| json        | 1.3x     | 167 ms     | python 129 ms | 1.8x        |
| sieve       | 1.3x     | 90 ms      | python 71 ms  | 2.0x        |
| mandelbrot  | 1.2x     | 224 ms     | python 184 ms | 1.4x        |
| hashmap_int | 1.2x     | 97 ms      | node 83 ms    | 1.7x        |

## On the floor, short of the target

Three cases where RustScript beats the slowest rival but not both. Gap is
against the fastest rival. `binary_trees` sits on the line, it trades places
with Python between runs.

| case         | gap to fastest | rustscript | fastest rival |
| ------------ | -------------- | ---------- | ------------- |
| word_count   | 1.5x           | 91 ms      | python 60 ms  |
| higher_order | 1.2x           | 72 ms      | python 59 ms  |
| binary_trees | 1.0x           | 31 ms      | python 30 ms  |

## At the target

Twelve cases beat both rivals on wall clock. All three startup cases,
`string_builder`, both sort cases, `json_serialize`, `stdout_lines`,
`file_transform`, `process_spawn`, `async_tasks`, and `http_local`.

`file_transform` got here through the scalar loop specialization in
`interpreter/scalar_loop.rs`. Its byte checksum loop, one dispatched
iteration per byte before, now runs unboxed inside one `ForNext` dispatch,
which took the case from 126 ms wall, the slowest of the three, to 37 ms,
ahead of both rivals.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead.

The work, grouped:

- Integer and float loops, `collatz`, `sieve`, `nbody`, `mandelbrot`, `fib`,
  `higher_order`. The scalar loop plan already runs int-only `for` bodies
  over bytes and ranges unboxed, which is what fixed `file_transform`. The
  losses left in this group are the shapes it does not cover yet: `while`
  loops, a function call per iteration, vec indexing inside the body, float
  arithmetic, and closure-driven adaptor chains. Extending the plan to those
  shapes is the direction.
- Per-line and per-token string work, `word_count`, `regex`, `automation`.
  The regex engine itself is the native `regex` crate, the cost is the
  interpreted loop around it that touches every line.
- Map and JSON traversal, `hashmap_int` and `json`. Boxed keys and values
  against C dict internals.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
