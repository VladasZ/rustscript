# Roadmap

Open problems from the differential runs between 2026-08-20 and 2026-08-28,
replayed against `0ad2cca`. 312 saved cases, 248 passed then, 3 are declared
gaps, 61 failed. Every failure found so far is fixed, and each one has its
regression case under `crates/differential/regressions`.

Each seed replays with `generate --seed N` only for the generator at the time
of the run. The old sources are in the run artifacts of the Differential
workflow.

## Open

Nothing open.

## Skip unless it comes back

Ambiguous numeric type, 17 cases. `.abs()`, `.log2()`, `.hypot()`,
`.is_nan()`, `.rem_euclid()`, `.rotate_left()` and similar called on an
unsuffixed literal. All from the 2026-08-20 and 2026-08-21 runs, none after
2026-08-22, so it looks fixed in the generator. Old sources cannot be
regenerated, so replay cannot prove it.
