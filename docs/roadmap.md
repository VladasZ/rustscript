# Roadmap

Plans only. This file holds open problems and nothing else. When a case is
fixed, delete its entry here. The history lives in git log and in the
regression cases under `crates/differential/regressions`.

A script session that hits a gap adds it as the first entry of `## Open`, since
it blocks that script, and the top entry is fixed first.

Each entry needs a minimal script that reproduces it, the compiled output next
to the interpreted output, and the likely place in the interpreter if known.
Differential seeds replay with `generate --seed N` only for the generator at
the time of the run. The old sources are in the run artifacts of the
Differential workflow.

## Open

### `PathBuf::push` does nothing

**Current.** A `push` on a `PathBuf` runs with no error and leaves the path
as it was. Found on 2026-10-08 in `build/shared/src/hot.rs` of hilen, the
function `relative`, which builds a path with `push` in a loop. It gave an
empty path, and the cargo line made from it named the wrong folder. The unit
test of that function passes, it is compiled.

```rust
use std::path::PathBuf;

fn main() {
    let mut a = PathBuf::new();
    a.push("x");
    println!("a `{}`", a.display());

    let mut b = PathBuf::from("x");
    b.push("y");
    println!("b `{}`", b.display());

    let mut c = PathBuf::from("x");
    c.push("..");
    println!("c `{}`", c.display());
}
```

```
compiled       interpreted
a `x`          a ``
b `x/y`        b `x`
c `x/..`       c `x`
```

`String::push_str` in the same kind of loop is right. The likely place is
where the method `push` is resolved by the type of its receiver,
`interpreter/compile/infer/methods.rs`, a `PathBuf` may fall into a branch
that has no write back to the variable.

**Blocks.** `make swap` of hilen for an app outside the hilen repo, the
script `build/ios/swap.rs`.

### The generator never makes 9 rows, and 2 expected features are near the edge

**Current.** The solver picks with the same chance among all catalog rows
that fit the wanted type, in `synth/exprs.rs` near line 402. A type that
few statements ask for, or that many rows give, makes a row rare or never
picked. Measured on 2026-10-07 with a probe test over the seeds 0 to 6000:

- 9 rows had 0 hits: `iter_by_ref`, `opt_zip`, `osp_map_strings`,
  `split_once`, `vec_binary_search`, `vec_enumerate`, `vec_split_at`,
  `vec_split_first` and `vec_split_last`. Their results are tuples or pairs
  that no statement seems to ask for. They are not in `EXPECTED_FEATURES`,
  so no test is red for them, and `every_catalog_method_is_reachable`
  accepts them.
- All 31 rows with a plain `usize` result are rare, 44 to 88 hits in 6000
  seeds and 0 to 10 in the 400 seeds of `generation_covers_the_language`.
  71 rows fit a wanted `usize`. `sl_len` has 0 hits in 400.
- After the fix of `iter_scan_running`, `str_bytes_lower_count` has 1 hit
  in 400 seeds and `iter_fold_print` has 3. Each change of a row shifts
  what every seed makes, so one of them can fall to 0 by chance.

`iter_scan_running` was red this way from 2026-10-04, its first seed was
415. It gives back the receiver type now, like `iter_rchunks`, and has 31
hits in 400.

**Needed.** A statement form that asks for the pair and tuple results of
the 9 rows, or those rows moved to a result the solver asks for. For the
rare class, a weight or a pick by result type first, so one row does not
share its chance with 70 others. Then a floor in the coverage test, a
feature with fewer than a few hits in 400 seeds fails with its count.

**Blocks.** Bugs in those 9 methods cannot be found by the generator. The
coverage test can go red by chance at any generator change.

## Generator plan

The differential generator is being brought closer to real Rust in phases,
so it reaches the semantics the interpreter models by hand and the idioms the
scripts in `thing` write. The ownership core, the drop tracer and the
references of phase 1 are done, see `docs/differential.md`. Each phase below ends the same way. New names in
`EXPECTED_FEATURES`, the compile guard green, a local campaign, and every
finding fixed in the interpreter with a promoted regression.

Real use breaks most often in the bridges and in exit codes, so phases 6 and
3 come before 4 and 5. Width probes, modules in the renderer, the script call
counts in the surface report and win1 in the fleet are done, see
`docs/differential.md`.

### Phase 2, the rest

The catalog rows of this phase are done, see `docs/differential.md`. A row is one fixed
template. Still open are the same forms as tree nodes the solver composes.

- Pipe sources from `iter()`, `chars()`, `bytes()`, `lines()`, `split()`, `windows`,
  `chunks`, and ranges with `rev` and `step_by`. Pipe stages `filter_map`, `flat_map`,
  `flatten`, `chain`, `zip`, `take_while`, `skip_while`, `inspect` with a print, `scan`,
  `peekable` and `by_ref`. Pipe terminals `find`, `find_map`, `max_by_key`, `min_by_key`,
  `partition`, `unzip`, `for_each`, `reduce`, `rposition`, `collect::<String>` and
  `collect::<Result<Vec<_>, _>>`. Each with a generated closure body.
- `BTreeMap`, `BTreeSet` and `VecDeque` as types a binding holds, so every statement form
  reads and writes them.
- `write!` and `writeln!` as statements over a `String` binding, and a `format!` whose
  arguments are generated expressions.
- A print inside the generated body of a `map`, `filter` or `fold` closure of a pipe.
- An inline format argument written with a keyword name, `println!("{t}", type = x)`, does
  not parse in the interpreter.

### Phase 3, error handling and process semantics

- `fn main() -> Result<(), E>` with `String`, a user error enum, and
  `Box<dyn Error>`. `?` in main, the `Error: ...` line and exit 1.
- `process::exit(n)` after buffered prints, `panic!` with a formatted
  message, `unwrap` and `expect` on `None` and `Err` with the std messages,
  `assert!` and `assert_eq!` failures with the left and right lines,
  `unreachable!`.
- `map_err`, `and_then`, `ok_or_else`, `unwrap_or_else` with a closure,
  `is_ok_and`, `is_some_and`, `transpose`, `?` through `From` chains.
- Runner: compare status, stdout and stderr for any exit code, and a new
  `ExitCodeMismatch` class. The batch renamer must handle a `main` that
  returns `Result`.

### Phase 4, user types the way people write them

- `&mut self` methods that write fields, tuple structs, unit structs, enums
  with struct variants, `Self::new`, methods returning `&T` and `Option<&T>`.
- Hand written `impl Default`, `PartialOrd`, `Ord`, `FromStr`, `Iterator`,
  `Add`, `Neg`, `Index`, `Drop`, and a printing `impl Clone` so a clone
  becomes a line of output like a drop is now. Trait definitions with default
  methods and generic bounds, `impl Trait` params and returns,
  `Box<dyn Trait>` in a `Vec`.
- `Box` recursive enums with recursive fns, `Rc<RefCell<T>>` shared graphs
  with a conflicting borrow that panics, `Cell`, `Rc::strong_count`. Statics,
  arrays `[T; N]`, `type` aliases, nested `mod` with `pub` items.

### Phase 5, closures and functions as values

Closures stored in a `Vec<Box<dyn Fn>>`, returned as `impl Fn`, capturing
`&mut` and called repeatedly, `FnOnce` consumption, fn items as values like
`map(u64::from)` and `ToString::to_string`, nested fns, direct and mutual
recursion with a depth.

### Phase 6, bridged crates and async

The runner links the bridged crates with `--extern` and a block reads json
documents into a random struct, both done. Still open are toml and yaml
documents, a nested struct and an enum as a field type, `serde_json::Value`
edits, `Regex` captures, and `#[tokio::main]` with `spawn`, `join!` and print
order. The `join!` flaw from `flaws.md` lives here.
