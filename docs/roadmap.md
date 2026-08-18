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
float and struct-field support in the scalar plans included, Node v26.7.0,
Python 3.14.7. Re-run the suite and update this file when the gaps change.

## Below the floor

Five cases where RustScript is the slowest of the three, worst first. Gap
is the RustScript median divided by the slowest rival's median. The compute
gap column shows the same ratio on the compute-only track, which tells how
much of the loss is raw interpreter speed rather than startup.

| case        | wall gap | rustscript | slowest rival | compute gap |
| ----------- | -------- | ---------- | ------------- | ----------- |
| regex       | 1.7x     | 176 ms     | python 107 ms | 2.3x        |
| fib         | 1.6x     | 97 ms      | node 62 ms    | 3.6x        |
| automation  | 1.4x     | 147 ms     | python 104 ms | 1.9x        |
| json        | 1.3x     | 164 ms     | python 131 ms | 1.9x        |
| hashmap_int | 1.2x     | 98 ms      | node 80 ms    | 1.8x        |

## On the floor, short of the target

Two cases where RustScript beats the slowest rival but not both. Gap is
against the fastest rival.

| case         | gap to fastest | rustscript | fastest rival |
| ------------ | -------------- | ---------- | ------------- |
| word_count   | 1.5x           | 92 ms      | python 60 ms  |
| higher_order | 1.2x           | 67 ms      | python 55 ms  |

## At the target

Seventeen cases beat both rivals on wall clock. All three startup cases,
`collatz`, `string_builder`, both sort cases, `json_serialize`,
`stdout_lines`, `file_transform`, `process_spawn`, `async_tasks`,
`http_local`, `sieve`, `binary_trees`, `mandelbrot`, and `nbody`.

`file_transform` got here through the scalar loop specialization in
`interpreter/scalar_for.rs`. Its byte checksum loop, one dispatched
iteration per byte before, now runs unboxed inside one `ForNext` dispatch,
which took the case from 126 ms wall, the slowest of the three, to 36 ms,
ahead of both rivals.

`collatz` got here through the scalar while loop specialization in
`interpreter/scalar_while.rs`, the whole `while` region including the
condition and the `is_multiple_of` call runs unboxed inside one dispatch.
It was the worst case on this list at 2.1x and 189 ms, now 54 ms, ahead of
both rivals.

`sieve` got here through vec indexing in while plans and vec sources in
`for` plans. Its marking loops, `v[i]` reads and journaled `v[i] = x`
writes on locked storage, and its counting loop, an `iter().skip(2)` walk
of the same vec, all run unboxed. It was 1.3x at 92 ms wall and 85 ms
compute, now 31 ms wall and 24 ms compute, ahead of both rivals.

`mandelbrot` got here through f64 support in the scalar plans: float
arithmetic, comparisons with NaN semantics, float literals, and `f64::from`
on the loop counters all run unboxed. It was 1.2x at 225 ms wall, now
60 ms, ahead of both rivals.

`nbody` got here through struct elements in vec while plans plus the float
work. Its pair loops, `bodies[i].x` reads and `bodies[i].vx -= e` writes
through element handles on locked storage, with `sqrt` from the plan's
float method table, run unboxed, and the `LoopHead` op hands every loop to
its plan at entry instead of after one generic iteration. It was the worst
compute case at 1.6x and 230 ms wall, now 62 ms, ahead of both rivals.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead.

The work, grouped:

- Calls and closures, `fib` and `higher_order`. The scalar plans run int
  and float `for`, `while`, and `loop` bodies unboxed, vec indexing, vec
  iteration, and struct fields in vecs included, which is what fixed
  `file_transform`, `collatz`, `sieve`, `mandelbrot`, and `nbody`. The
  losses left in this group are the shapes the plans do not cover yet: a
  function call per iteration, which blocks `fib`, and closure-driven
  adaptor chains, which block `higher_order`. Extending the plans to those
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
