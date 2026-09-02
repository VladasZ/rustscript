# Roadmap

Open problems from the differential runs between 2026-08-20 and 2026-08-28,
replayed against `0ad2cca`. 312 saved cases, 248 passed then, 3 are declared
gaps, 61 failed. Every failure found so far is fixed, and each one has its
regression case under `crates/differential/regressions`.

Each seed replays with `generate --seed N` only for the generator at the time
of the run. The old sources are in the run artifacts of the Differential
workflow.

## Open

Nothing open right now.

The missing `std::cmp::Reverse` was fixed on 2026-09-02. It builds the
`Reverse` newtype as a bridge struct, `sort_key` and value comparison flip
around it, guarded by the `cmp_reverse` example.

The missing `OsStr::to_str` was fixed on 2026-09-02. `Path::extension` and
`Path::file_stem` hand back plain strings, so `to_str` and `to_string_lossy`
went into the string method core, guarded by the `os_str_to_str` example.

The ambiguous numeric type family came back on 2026-09-02 as a `RustcRejected`
at seed 2852428022, a match arm binding a bare int scrutinee and calling
`saturating_pow` on it. Fixed at the root, a binding pattern now forces real
suffixes on the scrutinee, guarded by `bound_match_scrutinees_carry_their_widths`.
