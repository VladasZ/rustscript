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

### A `ref mut` binding over a place scrutinee binds a copy

`if let (ref mut x, _) = t { *x += 10 }` panics with `assignment through a
non-reference value`. `pattern_owns` is false for a `ref` pattern, so
`compile_scrutinee` in `compile/place.rs` reads `t` as a plain value and
`test_bind` in `vm_step/control.rs` takes the value path, which binds `x` to a
copy of the scalar. Fix by wrapping a place scrutinee in `MakeBorrow` when
`pattern_borrows` holds, like `&mut place`, so the reference path anchors the
bindings to the storage.

```rust
fn main() {
    let mut t = (1, 2);
    if let (ref mut x, _) = t {
        *x += 10;
    }
    println!("{t:?}");
}
```

Compiled prints `(11, 2)`. Interpreted panics at `*x += 10`.

### `?` on an owned temporary drops the payload it handed out

With any `Drop` impl in the program, `Ok::<Result<bool, E>, E>(Ok(false))?`
returns an `Ok` with no payload. The outer `Ok` is an owned temporary, so the
statement end drops it, and `run_user_drop` in `bridge/fmt_drop.rs` takes the
payload list out of its storage and drops through into the inner `Ok`, which
`try_op` in `vm_step/control.rs` had handed out as a shared handle. Fix by
taking the payload out of an owned receiver in `Try` and `TryJump`, leaving
unit behind, the way `TakeBinds` does for a pattern.

```rust
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn inner() -> Result<bool, String> {
    Ok::<Result<bool, String>, String>(Ok(false))?
}
fn main() {
    println!("{:?}", inner());
    println!("{}", T(1).0);
}
```

Compiled prints `Ok(false)`, `1`, `drop 1`. Interpreted prints `Ok`, `1`,
`drop 1`. Found by a local campaign at seed 904174 with the generator of
v0.6.30.

### Temporaries alive at a panic do not drop during the unwind

A method receiver that owns a value is not dropped when an argument panics.
`unwind_temps` in `compile/calls.rs` only records owned call arguments, so
the receiver and the other owned temporaries of the statement are not on
the list the panic path drops. Fix by dropping the live `owned_temps` of the
frame on the unwind path as well, newest first.

```rust
#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let empty: Vec<T> = Vec::new();
    println!("{:?}", Ok::<T, String>(T(0)).unwrap_or(empty[2].clone()));
}
```

Both panic with `index out of bounds: the len is 0 but the index is 2`.
Compiled prints `drop 0` during the unwind. Interpreted prints nothing. Found
by a local campaign with the generator of v0.6.30, seeds 904025, 904057,
904137, 904149, 904154, 904192, 904221 and 904256 from base seed 904000 are
all this class.

## Generator plan

The differential generator is being brought closer to real Rust in phases,
so it reaches the semantics the interpreter models by hand and the idioms the
scripts in `thing` write. Phase 1, the ownership core and the drop tracer, is
done, see `docs/differential.md`. Each phase below ends the same way. New
names in `EXPECTED_FEATURES`, the compile guard green, a local campaign, and
every finding fixed in the interpreter with a promoted regression.

### Phase 2, references and binding forms

- `&str` as a real type. Literals are `&'static str`, borrows of a `String`
  local live in a frozen region. Fn params `&str` and `&[T]`, calls with `&v`,
  `&v[1..3]`, `s.as_str()`, `&s[..2]`. `Option<&str>` from `strip_prefix`
  and `split_once`.
- `for x in &v`, `for x in v.iter()`, `for (k, v) in &map` through the sort
  rule, `for (i, x) in v.iter().enumerate()`, `iter_mut` with any write
  through `*x` or a method.
- `&mut` through `get_mut`, `last_mut`, `entry().or_insert_with`,
  `values_mut`, `&mut s.f0`, `&mut v[i]`, and a borrow block
  `{ let r = &mut x; ... }` that freezes `x`.
- Closure params by reference with `|&b|` and `|b| *b != 0`. This is the
  `filter` over `Vec<u8>` bug class from `rustscript-flaws.md`.
- `if let`, `while let Some(x) = v.pop()`, `let else`, let chains, `match`
  as a statement with pushes, assigns, prints, `break`, `continue` and
  `return` in arms, `loop { break value }`, or patterns, `@` bindings, `ref`
  and `ref mut`, nested patterns, const patterns, string literal arms on
  `.as_str()`, `matches!`.

### Phase 3, strings, formatting and iterators as scripts write them

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

### Phase 4, error handling and process semantics

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

### Phase 5, user types the way people write them

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

### Phase 6, closures and functions as values

Closures stored in a `Vec<Box<dyn Fn>>`, returned as `impl Fn`, capturing
`&mut` and called repeatedly, `FnOnce` consumption, fn items as values like
`map(u64::from)` and `ToString::to_string`, nested fns, direct and mutual
recursion with a depth.

### Phase 7, bridged crates and async

The runner links prebuilt rlibs from the examples crate with `--extern`, so
`serde_json`, `regex`, `chrono` and `tokio` programs compile without cargo
per case. Then `serde_json::Value` edits, typed `from_str` with inference
sites, `Regex` captures, `#[tokio::main]` with `spawn`, `join!` and print
order. The `join!` flaw from `rustscript-flaws.md` lives here.
