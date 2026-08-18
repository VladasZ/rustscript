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
closure chain plans included, Node v26.7.0, Python 3.14.7. Re-run the
suite and update this file when the gaps change.

## Below the floor

Three cases where RustScript is the slowest of the three, worst first. Gap
is the RustScript median divided by the slowest rival's median. The compute
gap column shows the same ratio on the compute-only track, which tells how
much of the loss is raw interpreter speed rather than startup.

| case        | wall gap | rustscript | slowest rival | compute gap |
| ----------- | -------- | ---------- | ------------- | ----------- |
| automation  | 1.4x     | 148 ms     | python 106 ms | 1.9x        |
| json        | 1.3x     | 167 ms     | python 128 ms | 1.8x        |
| hashmap_int | 1.2x     | 96 ms      | node 80 ms    | 1.9x        |

## On the floor, short of the target

One case where RustScript beats the slowest rival but not both. Gap is
against the fastest rival.

| case       | gap to fastest | rustscript | fastest rival |
| ---------- | -------------- | ---------- | ------------- |
| word_count | 1.5x           | 93 ms      | python 61 ms  |

## At the target

Twenty cases beat both rivals on wall clock. All three startup cases,
`fib`, `collatz`, `string_builder`, both sort cases, `json_serialize`,
`stdout_lines`, `file_transform`, `process_spawn`, `async_tasks`,
`http_local`, `sieve`, `binary_trees`, `mandelbrot`, `nbody`, `regex`,
and `higher_order`.

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

`higher_order` got here through two pieces. The closure chain plans in
`interpreter/scalar_chain.rs` run a `sum`, `count`, `any`, or `all`
driving `map` and `filter` stages unboxed: each closure body translates
once into a plan, and the whole chain runs element by element with no
boxed value and no closure frame anywhere. And vec `push` in the for
plans runs its fill loop, `v.push(x % 1000)` over a range, unboxed with
truncate-based undo. It was 1.2x at 71 ms wall, now 16 ms, ahead of both
rivals.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead.

The work, grouped:

- Per-line and per-token string work, `word_count` and `automation`. The
  scalar plans now run loop bodies, vec indexing, struct fields in vecs,
  whole self-recursive function bodies, regex match loops, closure
  adaptor chains, and vec pushes, which is what fixed every numeric case
  and `higher_order`. The loss left is the interpreted loop that touches
  every line and token, boxed strings included. Extending the plans to
  string items is the direction.
- Map and JSON traversal, `hashmap_int` and `json`. Boxed keys and values
  against C dict internals.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
