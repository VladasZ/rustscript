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
regex loop plans included, Node v26.7.0, Python 3.14.7. This run happened
with light desktop activity. Re-run the suite and update this file when
the gaps change.

## Below the floor

Three cases where RustScript is the slowest of the three, worst first. Gap
is the RustScript median divided by the slowest rival's median. The compute
gap column shows the same ratio on the compute-only track, which tells how
much of the loss is raw interpreter speed rather than startup.

| case        | wall gap | rustscript | slowest rival | compute gap |
| ----------- | -------- | ---------- | ------------- | ----------- |
| automation  | 1.4x     | 151 ms     | python 107 ms | 1.8x        |
| hashmap_int | 1.3x     | 123 ms     | python 91 ms  | 1.8x        |
| json        | 1.3x     | 173 ms     | python 134 ms | 1.8x        |

## On the floor, short of the target

Two cases where RustScript beats the slowest rival but not both. Gap is
against the fastest rival.

| case         | gap to fastest | rustscript | fastest rival |
| ------------ | -------------- | ---------- | ------------- |
| word_count   | 1.6x           | 97 ms      | python 62 ms  |
| higher_order | 1.2x           | 71 ms      | python 60 ms  |

## At the target

Nineteen cases beat both rivals on wall clock. All three startup cases,
`fib`, `collatz`, `string_builder`, both sort cases, `json_serialize`,
`stdout_lines`, `file_transform`, `process_spawn`, `async_tasks`,
`http_local`, `sieve`, `binary_trees`, `mandelbrot`, `nbody`, and
`regex`.

`fib` got here through the scalar function plans in
`interpreter/scalar_fn.rs`. A self-recursive function whose whole body
compiles to scalar bytecode runs its entire call tree unboxed inside one
`CallFn` dispatch, on a flat frame stack with no boxed `Value` anywhere.
It was 1.6x at 97 ms wall, the second worst case on the list, now 38 ms,
ahead of both rivals.

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

`regex` got here through `find_iter` sources in the for plans. Its match
loop, 250k matches each paying a boxed match, a `m.start()` dispatch walk,
and a boxed `i64::try_from(..).unwrap()` result, now runs unboxed: the
match is a span slot, and the span read, the conversion, and the unwrap
are plan ops. It was the worst case on this list at 1.7x and 191 ms wall,
now 56 ms, ahead of both rivals.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead.

The work, grouped:

- Closures, `higher_order`. The scalar plans run int and float loop
  bodies unboxed, vec indexing, struct fields in vecs, whole
  self-recursive function bodies, and now regex match loops, which is
  what fixed `file_transform`, `collatz`, `sieve`, `mandelbrot`, `nbody`,
  `fib`, and `regex`. The loss left in this group is closure-driven
  adaptor chains, `map`, `filter`, `sum` over a vec with a closure call
  per element. Extending the plans to closure bodies is the direction.
- Per-line and per-token string work, `word_count` and `automation`. The
  cost is the interpreted loop that touches every line and token, boxed
  strings included.
- Map and JSON traversal, `hashmap_int` and `json`. Boxed keys and values
  against C dict internals.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
