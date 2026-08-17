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
`bench/results/results.json`, one run on an Apple M1 Pro, 2026-08-17, commit
`28970bc`, Node v26.7.0, Python 3.14.7. Re-run the suite and update this
file when the gaps change.

## Below the floor

Ten cases where RustScript is the slowest of the three, worst first. Gap is
the RustScript median divided by the slowest rival's median. The compute gap
column shows the same ratio on the compute-only track, which tells how much
of the loss is raw interpreter speed rather than startup.

| case           | wall gap | rustscript | slowest rival   | compute gap |
| -------------- | -------- | ---------- | --------------- | ----------- |
| collatz        | 2.2x     | 196 ms     | python 91 ms    | 3.0x        |
| regex          | 1.7x     | 181 ms     | python 107 ms   | 2.2x        |
| nbody          | 1.7x     | 237 ms     | python 144 ms   | 2.1x        |
| fib            | 1.5x     | 102 ms     | node 68 ms      | 3.6x        |
| sieve          | 1.4x     | 108 ms     | python 74 ms    | 2.0x        |
| automation     | 1.4x     | 149 ms     | python 106 ms   | 1.8x        |
| file_transform | 1.3x     | 126 ms     | node 99 ms      | 4.0x        |
| json           | 1.2x     | 173 ms     | python 139 ms   | 1.8x        |
| mandelbrot     | 1.2x     | 232 ms     | python 194 ms   | 1.4x        |
| hashmap_int    | 1.2x     | 97 ms      | node 83 ms      | 1.8x        |

## On the floor, short of the target

Two cases where RustScript beats the slowest rival but not both. Gap is
against the fastest rival.

| case         | gap to fastest | rustscript | fastest rival |
| ------------ | -------------- | ---------- | ------------- |
| word_count   | 1.5x           | 95 ms      | python 63 ms  |
| higher_order | 1.3x           | 74 ms      | python 59 ms  |

## At the target

Twelve cases already beat both rivals on wall clock. All three startup
cases, `binary_trees`, `string_builder`, both sort cases, `json_serialize`,
`stdout_lines`, `process_spawn`, `async_tasks`, and `http_local`.

## Why the failing cases lose

The pattern is one pattern. Python and Node win exactly where their hot path
leaves their interpreter. CPython runs `re`, `json`, string methods, and
dict internals as C code, and V8 JIT compiles hot loops to machine code.
RustScript executes the same loop as bytecode, one dispatch per operation,
with every value behind an `Arc`. The startup lead absorbs roughly 25 ms of
that against Python and 60 ms against Node, so a case fails exactly when the
compute loss grows past the lead. `binary_trees` loses 4.5x on compute and
still beats both rivals on wall clock, while `collatz` loses 3.0x on a
longer workload and lands at the bottom.

The work, grouped:

- Integer and float loops, `collatz`, `sieve`, `nbody`, `mandelbrot`, `fib`,
  `higher_order`. Pure dispatch overhead against CPython's specializing
  interpreter. The scalar specialization that fixed `sort` shows the
  direction, recognize int-only bodies and run them on flat registers
  without boxing.
- Per-line and per-token string work, `file_transform`, `word_count`,
  `regex`, `automation`. The regex engine itself is the native `regex`
  crate, the cost is the interpreted loop around it that touches every line.
- Map and JSON traversal, `hashmap_int` and `json`. Boxed keys and values
  against C dict internals.

## Not goals

Beating native Rust is out of scope, `rust build` exists for that. Winning
the compute-only track against V8 is out of scope too, the goal is the whole
run.
