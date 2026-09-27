# Roadmap

Plans only. This file holds open problems and nothing else. When a case is
fixed, delete its entry here. The history lives in git log and in the
regression cases under `crates/differential/regressions`.

Each entry needs a minimal script that reproduces it, the compiled output next
to the interpreted output, and the likely place in the interpreter if known.
Differential seeds replay with `generate --seed N` only for the generator at
the time of the run. The old sources are in the run artifacts of the
Differential workflow.

## Open

Nothing open.

## Generator plan

The differential generator is being brought closer to real Rust in phases,
so it reaches the semantics the interpreter models by hand and the idioms the
scripts in `thing` write. The ownership core and the drop tracer are done,
see `docs/differential.md`. Each phase below ends the same way. New names in
`EXPECTED_FEATURES`, the compile guard green, a local campaign, and every
finding fixed in the interpreter with a promoted regression.

### Phase 1, references

The binding forms are done, see `docs/differential.md`. What is left.

- `&str` as a real type. Literals are `&'static str`, borrows of a `String`
  local live in a frozen region. Fn params `&str` and `&[T]`, calls with `&v`,
  `&v[1..3]`, `s.as_str()`, `&s[..2]`. `Option<&str>` from `strip_prefix`
  and `split_once`. String literal arms on `.as_str()`. The model is in place
  and nothing generates it yet. `Ty::StrRef`, `Ty::Slice`, `Expr::Borrow` and
  `Pat::StrLit` render, and `own_check` holds the borrow rules. The generator
  still needs an `expr` road for a reference type, the `let` that keeps one,
  the catalog rows that take and hand out `&str`, and the fn params.
- `for x in &v`, `for x in v.iter()`, `for (k, v) in &map` through the sort
  rule, `for (i, x) in v.iter().enumerate()`, `iter_mut` with any write
  through `*x` or a method.
- `&mut` through `get_mut`, `last_mut`, `entry().or_insert_with`,
  `values_mut`, `&mut s.f0`, `&mut v[i]`, and a borrow block
  `{ let r = &mut x; ... }` that freezes `x`. A `ref mut` binding belongs
  here too.
- Closure params by reference with `|&b|` and `|b| *b != 0`. This is the
  `filter` over `Vec<u8>` bug class from `rustscript-flaws.md`.

### Phase 2, strings, formatting and iterators as scripts write them

- `format!` with positional, named and inline args, nested specs, `write!`
  and `writeln!` into a `String`, `+` and `+=` with `&str`, `to_string`
  against `String::from` against `into`, `chars().rev()`, `char_indices`,
  `bytes`, `lines`, `split` with `map(str::trim)`, `parse::<T>()` with `?`
  and `map_err`.
- Pipe sources from `iter()`, `chars()`, `bytes()`, `lines()`, `split()`,
  `windows`, `chunks`, ranges with `rev` and `step_by`. New stages
  `filter_map`, `flat_map`, `flatten`, `chain`, `zip`, `take_while`,
  `skip_while`, `inspect` with a print, `scan`, `peekable` driven by
  `while let`, `by_ref`, and an iterator stored in a binding and pulled with
  `next`. New terminals `find`, `find_map`, `max_by_key`, `min_by_key`,
  `partition`, `unzip`, `for_each`, `reduce`, `rposition`,
  `collect::<String>`, `collect::<Result<Vec<_>, _>>`.
- `BTreeMap`, `BTreeSet` and `VecDeque`. They are ordered, so they print
  directly and the sort rule does not apply.
- `sort_by_key`, `sort_by` with `cmp` and `Reverse`, `dedup_by_key`,
  `binary_search`, `drain`, `split_off`, `insert`, `remove`, `extend` from
  an iterator, `and_modify`.
- Prints inside `map`, `filter`, `fold`, `for_each` and helper bodies. Never
  inside a sort comparator, its call sequence is not part of the std
  contract.
- Evaluation order tracers, a helper that prints and returns its argument,
  placed in call args, operands, struct fields, index and value of an
  assignment.

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

The runner links prebuilt rlibs from the examples crate with `--extern`, so
`serde_json`, `regex`, `chrono` and `tokio` programs compile without cargo
per case. Then `serde_json::Value` edits, typed `from_str` with inference
sites, `Regex` captures, `#[tokio::main]` with `spawn`, `join!` and print
order. The `join!` flaw from `rustscript-flaws.md` lives here.
