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

The ambiguous numeric type family came back on 2026-09-02 as a `RustcRejected`
at seed 2852428022, a match arm binding a bare int scrutinee and calling
`saturating_pow` on it. Fixed at the root, a binding pattern now forces real
suffixes on the scrutinee, guarded by `bound_match_scrutinees_carry_their_widths`.
