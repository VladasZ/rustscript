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

### An earlier call argument unwinds after a later temporary

When an argument panics, real Rust drops the arguments already evaluated
before it drops the temporaries made inside the panicking argument. The
interpreter drops the whole frame highest register first in `unwind_drops` in
`vm.rs`, so the later temporary goes first. Fix by keeping the owned call
arguments of the window, `unwind_temps` in `compile/calls.rs`, ahead of the
other droppable registers of their statement, or by recording the order the
compiler wants on the chunk instead of sorting by register.

```rust
#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn take(_a: T, _b: &T) -> i64 {
    0
}
fn main() {
    let v = take(T(1), &vec![T(2)][2].clone());
    println!("{v}");
}
```

Both panic with `index out of bounds: the len is 1 but the index is 2`.
Compiled prints `drop 1` then `drop 2` during the unwind. Interpreted prints
`drop 2` then `drop 1`. Found by a local campaign at seed 20705000113 with the
generator of v0.6.31.

### A value lent to a panicking call never drops

`enter` in `vm.rs` moves every argument out of the caller's window into the
callee's registers, and the caller's local was cleared by `emit_borrow_takes`
so `Rc::strong_count` stays right. A `&T` or `&mut T` parameter is not in the
callee's `droppable` list, and the writeback that would return the value to
the caller's local only runs on a normal return. So when the callee panics,
the lent value is in no list and never drops. Fix by giving the unwind the
call site's writeback map, window slot to local register, and restoring the
lent values into the caller's locals before the caller's frame drops them,
so they drop at their declaration position like in real Rust.

```rust
#[derive(Debug, Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn write(t: &mut T) {
    let cur = T(t.0 + 10);
    *t = vec![T(5)][2].clone();
    println!("{}", cur.0);
}
fn main() {
    let mut a = T(1);
    let keep = T(9);
    write(&mut a);
    println!("{}", keep.0);
}
```

Both panic with `index out of bounds: the len is 1 but the index is 2`.
Compiled prints `drop 5`, `drop 11`, `drop 9`, `drop 1`. Interpreted stops
after `drop 9`. A `&a` argument and a `&T::new()` argument lose their drop the
same way. Found by a local campaign at seed 20705000321 with the generator of
v0.6.31, and most of the `drop N` findings left after v0.6.32 are this class
or the unwind order above.

### A bare `ref mut` binding over a scalar local binds a copy

`if let ref mut m = n { *m += 1 }` panics with `assignment through a
non-reference value`. A `ref mut` pattern over a tuple, a struct or an element
borrows the place since v0.6.32, see `compile_scrutinee` in
`compile/place.rs`, but a scalar local is its register and a register has no
reference form, so that path is skipped for it. Fix by aliasing the binding to
the local at compile time, the way `compile_let_borrow` handles
`let r = &mut n`.

```rust
fn main() {
    let mut n = 5;
    if let ref mut m = n {
        *m += 1;
    }
    println!("{n}");
}
```

Compiled prints `6`. Interpreted panics at `*m += 1`.

### The generator writes programs rustc rejects

About 5 of every 20000 cases fail to compile, so the runner counts them as
`RustcRejected` and learns nothing from them. The 4 shapes seen in the
Differential runs of 2026-09-09, run 34323849776, all with the generator of
v0.6.31:

- A match arm yields a local of the wrong type, `expected bool, found usize`
  from an arm that reads a `usize` binding where the match is typed `bool`.
  Seed 20705102828.
- A `String` binding of a pattern is moved inside the pattern guard,
  `cannot move out of ... in pattern guard`, when the guard builds a map or
  matches on the binding by value. Seeds 20705109085 and 20705201571.
- A rebinding `let mut v: char = v;` reads the outer `v` whose type is
  `Vec<i32>`. The shadowing `let` takes its annotation from the new type but
  its init from the old binding. Seed 20705204644.
- A map insert keys with a binding of another width, `expected i8, found i16`
  from `diff_map.insert(v_0_0, ..)` where the map is keyed by `i8`. Seed
  20705111020.

The sources are in the `differential-failures-*` artifacts of that run. Each
shape needs a typing rule in `lang/synth`, and the guard case needs
`own_check` to treat a guard as a borrow of its bindings.

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
