# Roadmap

Open problems from the differential runs between 2026-08-20 and 2026-08-28,
replayed against `0ad2cca`. 312 saved cases, 248 passed then, 3 are declared
gaps, 61 failed. Batches 1 to 3 below are done, every seed in them replays as
a match or, where the artifact schema is too old to replay, compares equal by
hand. Each batch has its regression cases under
`crates/differential/regressions`.

Each seed replays with `generate --seed N` only for the generator at the time
of the run. The old sources are in the run artifacts of the Differential
workflow.

## Open

Nothing open.

## Done

### Batch 4, mutation through `Option::as_mut`

Found 2026-08-29 in hilen `build/ui-test.rs`, not by the generator. Writing
through the binding of `if let Some((_, body)) = current.as_mut()` did not
reach the option, `as_mut` returned a copy of the enum so the `if let` bound
copies. `as_mut` and `as_deref_mut` on `Option` and `Result` return a borrow
now, in `interpreter/methods.rs`, so the bindings anchor to the payload.

Found beside it: `let (a, b) = &mut pair; a.push('x')` also wrote into a
copy, a destructuring `let` of a `&mut` place compiled the init as an owned
value. It compiles as a scrutinee now, `compile/block.rs`. Both are covered
by `crates/examples/examples/option_as_mut_pattern.rs`.

### Batch 1, f32 precision in the interpreter

1. `sum` and `product` over `f32` ran at f64 precision. Both now accumulate
   in the width the turbofish or the elements say, in `iterator/arith.rs`.
2. `[MAX, MAX, NEG_INFINITY]` summed to `-inf` instead of `NaN`, the same fix.

### Batch 2, remaining interpreter bugs

3. `E::from(pick(..))` picked the impl of the caller's expectation instead of
   what the generic call returns. The arguments bind the generic first now.
4. `"99999999999999999999\n".parse::<usize>()` said invalid digit instead of
   overflow. Integer parse runs the digit loop of std.
5. `saturating_pow(4294967294)` looped the exponent and timed out. `pow` and
   its family square.
6. `vec.into_iter().map(..).skip(n).collect::<Vec<_>>()` with `n` past the
   end ran the closure. The in place collect of std is modelled in
   `iterator/in_place.rs`.
7. A `repeat` count past `i64::MAX` saturated to `isize::MAX` and aborted on
   allocation instead of the `capacity overflow` panic.

Found beside batch 2: `rev` drained eagerly, so `map` closures ran in forward
order and a `skip` after `rev` evaluated the skipped element. `rev` is lazy
now, `iterator/back.rs`.

### Batch 3, generator

8. A `move` closure that named a factory closure did not treat it as moved,
   the binding carries the return type which is `Copy`. Closure bindings
   leave the scope now. The E0499 and E0502 seeds generate different
   programs at HEAD and did not reproduce.
9. The mutator could graft any `usize` expression into a `SmallUsize` slot,
   so `repeat` got a count that kills the native binary. Those argument
   subtrees are pinned, `Expr::pinned_nodes`.

## Skip unless it comes back

10. Ambiguous numeric type, 17 cases. `.abs()`, `.log2()`, `.hypot()`,
    `.is_nan()`, `.rem_euclid()`, `.rotate_left()` and similar called on an
    unsuffixed literal. All from the 2026-08-20 and 2026-08-21 runs, none
    after 2026-08-22, so it looks fixed in the generator. Old sources cannot
    be regenerated, so replay cannot prove it.

## Declared gaps, not bugs

- `next_power_of_two` on integers is unsupported. Seed 20686212010.
