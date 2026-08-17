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
`bench/results/results.json`, one run on an Apple M1 Pro, 2026-08-18, with
the scalar while loop specialization included, Node v26.7.0, Python 3.14.7.
Re-run the suite and update this file when the gaps change.

## Below the floor

Eight cases where RustScript is the slowest of the three, worst first. Gap
is the RustScript median divided by the slowest rival's median. The compute
gap column shows the same ratio on the compute-only track, which tells how
much of the loss is raw interpreter speed rather than startup.

| case        | wall gap | rustscript | slowest rival | compute gap |
| ----------- | -------- | ---------- | ------------- | ----------- |
| regex       | 1.7x     | 179 ms     | python 106 ms | 2.3x        |
| fib         | 1.6x     | 99 ms      | node 63 ms    | 3.6x        |
| nbody       | 1.6x     | 232 ms     | python 142 ms | 2.0x        |
| automation  | 1.4x     | 153 ms     | python 107 ms | 1.9x        |
| sieve       | 1.3x     | 92 ms      | python 72 ms  | 1.8x        |
| hashmap_int | 1.3x     | 107 ms     | node 85 ms    | 1.9x        |
| json        | 1.3x     | 169 ms     | python 133 ms | 1.9x        |
| mandelbrot  | 1.2x     | 227 ms     | python 193 ms | 1.4x        |

## On the floor, short of the target

Two cases where RustScript beats the slowest rival but not both. Gap is
against the fastest rival.

| case         | gap to fastest | rustscript | fastest rival |
| ------------ | -------------- | ---------- | ------------- |
| word_count   | 1.5x           | 94 ms      | python 61 ms  |
| higher_order | 1.2x           | 68 ms      | python 56 ms  |

## At the target

Fourteen cases beat both rivals on wall clock. All three startup cases,
`collatz`, `string_builder`, both sort cases, `json_serialize`,
`stdout_lines`, `file_transform`, `process_spawn`, `async_tasks`,
`http_local`, and `binary_trees`, which sits on the line and trades places
with Python between runs.

`file_transform` got here through the scalar loop specialization in
`interpreter/scalar_loop.rs`. Its byte checksum loop, one dispatched
iteration per byte before, now runs unboxed inside one `ForNext` dispatch,
which took the case from 126 ms wall, the slowest of the three, to 37 ms,
ahead of both rivals.

`collatz` got here through the scalar while loop specialization in the same
module, the whole `while` region including the condition and the
`is_multiple_of` call runs unboxed inside one dispatch. It was the worst
case on this list at 2.1x and 189 ms, now 52 ms, ahead of both rivals. The
same plan also cut `mandelbrot`'s compute by about a tenth through the
integer part of its escape loop.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead.

The work, grouped:

- Integer and float loops, `sieve`, `nbody`, `mandelbrot`, `fib`,
  `higher_order`. The scalar plans already run int-only `for`, `while`, and
  `loop` bodies unboxed, which is what fixed `file_transform` and `collatz`.
  The losses left in this group are the shapes the plans do not cover yet:
  vec indexing inside the body, which blocks `sieve` and `mandelbrot`, a
  function call per iteration, which blocks `fib`, float arithmetic, which
  blocks `nbody` and the rest of `mandelbrot`, and closure-driven adaptor
  chains. Extending the plans to those shapes is the direction.
- Per-line and per-token string work, `word_count`, `regex`, `automation`.
  The regex engine itself is the native `regex` crate, the cost is the
  interpreted loop around it that touches every line.
- Map and JSON traversal, `hashmap_int` and `json`. Boxed keys and values
  against C dict internals.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
