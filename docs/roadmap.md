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

All of these came out of the phase 1 campaigns. Each script below is the whole
repro, run it compiled and with `rust` to see the 2 outputs.

### A jump out of a statement leaves its temporaries alive

`break`, `continue` and `return` drop the scopes they leave, but not the
temporaries of the statements they leave. A `while let Some(_) = v.pop()` with
a `continue` never drops the popped value. A `match` scrutinee temporary is
lost when an arm leaves with `continue`, `break` or `return`.

```rust
#[derive(Debug, PartialEq)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn yes() -> bool {
    true
}

fn main() {
    let mut stack = vec![Trace(1), Trace(2)];
    while let Some(_) = stack.pop() {
        if yes() {
            continue;
        }
    }
    println!("after while let");
    for turn in 0..1i64 {
        let local = Trace(10 + turn);
        match Trace(20 + turn) == Trace(30 + turn) {
            false => {
                let inner = Trace(40 + turn);
                if yes() {
                    continue;
                }
                println!("unreachable {}", inner.0);
            }
            true => println!("equal {}", local.0),
        }
    }
    println!("end");
}
```

Compiled prints `drop 2`, `drop 1`, `after while let`, `drop 40`, `drop 30`,
`drop 20`, `drop 10`, `end`. Interpreted prints `after while let`, `drop 40`,
`drop 10`, `end`. The same holds for `break value` and for `return` inside a
`match` arm. Real Rust drops in this order, the scopes inside the statement,
then the temporaries of the statement, then the scope around it.

Likely place, `compile_break`, `compile_continue`, `emit_loop_exit_drops` and
`compile_return` in `compile/flow.rs`. They only walk `scope_order`. The
temporaries sit in `owned_temps` and the jump skips the `drop_temps` of their
statement. A fix needs the `owned_temps` length at each `push_scope` and at
loop entry, so an exit can drop the temporaries of every scope it leaves
between the scope drops.

### A moved local assigned again inside a match arm drops early

```rust
#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn pair() -> (u8, i64) {
    (0, 0)
}

fn main() {
    let mut first = Trace(1);
    let second = first;
    match pair() {
        (a, b) => {
            first = Trace(2);
        }
    }
    println!("{first:?} {second:?}");
}
```

Compiled prints `Trace(2) Trace(1)`, `drop 1`, `drop 2`. Interpreted prints
`drop 1` first, then the line, then `drop 1` and `drop 2`. The store into
`first` drops an old value that already moved to `second`, so `Trace(1)` drops
twice. The same assignment outside the `match` is fine. Likely place, the
liveness pass that turns the read in `let second = first` into a copy when a
later path may read `first` again, `resolve_owns` in `compile/fn_state.rs` and
`compile/liveness.rs`.

### `step_by` followed by `rev` runs the chain from the front

```rust
fn main() {
    let seen: Vec<i32> = (0..6)
        .map(|n| {
            println!("make {n}");
            n
        })
        .step_by(2)
        .rev()
        .collect();
    println!("{seen:?}");
}
```

Compiled prints `make 5` down to `make 0`. Interpreted prints `make 0` up to
`make 5`. The result is the same. Likely place, `supports_back` and
`back_step` in `iterator/back.rs` have no `StepBy` arm, so `rev` falls back to
draining the chain forward.

### `partition`, `max_by_key` and `min_by_key` lose items when the closure panics

```rust
#[derive(Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn three() -> usize {
    3
}

fn main() {
    let parts: (Vec<Trace>, Vec<Trace>) = vec![Trace(1), Trace(2), Trace(3)]
        .into_iter()
        .partition(|item| item.0 == 1 || Vec::<bool>::new()[three()]);
    println!("{parts:?}");
}
```

Compiled prints `drop 2`, `drop 3`, `drop 1`. Interpreted prints `drop 3`
alone. The item in flight and the items already sorted into the 2 vecs are
never dropped. `max_by_key` with a panicking key closure shows the same,
compiled `drop 2`, `drop 1`, `drop 3`, interpreted `drop 3`. Likely place, the
`Partition` and `MaxByKey` arms in `iterator/reduce.rs`. The adapters already
go through `call_lending` in `iterator/drive.rs`, the terminals do not, and
they also hold the items they kept in plain Rust vecs the unwinder never sees.

### `inspect` is not implemented

`vec![1, 2].into_iter().inspect(|n| println!("{n}")).count()` stops with
`inspect is not implemented by the interpreter`. Phase 2 of the generator plan
needs it.

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
