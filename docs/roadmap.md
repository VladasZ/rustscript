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

## Fixed

### A `const` as a match pattern never matches

A named constant used as a pattern in a `match` arm ran and never matched, the
value fell through to `_`. Hit in `shell/win/setup-run.rs` in the thing repo,
where the `Some(REBOOT_EXIT)` arm was dead and a reboot request was reported
as FAILED.

A pattern that names a constant now loads that constant into a register right
before the `TestBind` and compares by value, `PPat::Const`. A variant of the
same name still wins, so `None` stays a variant test. Module constants, block
constants, impl constants like `Type::LIMIT` and integer bounds like
`i32::MAX` all work, in every pattern position. Regression case
`const_as_match_pattern.rs`, example `const_patterns.rs`.

### `next_power_of_two` was missing its checked form

`next_power_of_two` itself worked on every unsigned width. The gap was
`checked_next_power_of_two`, which the interpreter rejected as not
implemented. It returns `None` past the top power now, and the u128 path goes
through the big integer pipeline. The catalog generates it, so the
differential covers it. Regression case `next_power_of_two_overflow.rs`,
example `power_of_two.rs`.

`wrapping_next_power_of_two` stays out. It is still unstable in std, so a
script using it cannot pass `cargo check` and no example could cover it.
