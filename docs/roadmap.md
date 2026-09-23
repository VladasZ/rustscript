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

### A labeled block `break` value is never dropped

```rust
struct Trace(i64);
impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let got = 'b: {
        break 'b Trace(9);
    };
    println!("got: {got:?}");
}
```

Compiled prints `got: Trace(9)` then `drop 9`. Interpreted prints `got:
Trace(9)` and never drops it. A `loop { break Trace(9); }` result drops fine, so
this is specific to the labeled block. Likely place: `compile_labeled_block` in
`compile/flow.rs` does not register its result register as an owned local for the
scope end drop, unlike `compile_loop`.

### Drops during a panic unwind miss the method receiver temporary

```rust
#[derive(Clone)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let v = vec![T(1); 3].get(1).cloned().unwrap_or(vec![T(2), T(3)][4].clone());
    println!("{}", v.0);
}
```

The `unwrap_or` argument `vec![T(2), T(3)][4]` panics with an index out of
bounds. Compiled drops during the unwind are `1, 2, 3, 1, 1, 1`. Interpreted
drops are `2, 3, 1, 1, 1`, missing the `.cloned()` receiver `Some(T(1))` and in
the wrong order. When a method argument panics, the already evaluated receiver
temporary is not on the unwind drop list, and the order of the remaining temps
does not match. Seeds 20718002141, 20718003167, 20718003168, 20718103155,
20718103156, 20718103305, 20718200322, 20718205510, 20718207034 are this class.

### An early spurious panic on a closure eval order divergence

Seeds 20718007971, 20718007972, 20718201125, 20718219778 and the
`InterpreterUnsupported` seed 20718106939. The programs build closures as
`impl Fn` and call them through a `&mut F` helper. Compiled runs to a later
index out of bounds panic. Interpreted panics early inside a closure body with
an empty message, so the two diverge before the real panic. Needs reduction with
`rustscript-differential replay` and `reduce` on one of the seeds to find the
eval order or closure dispatch difference. The failure artifacts are in the
Differential run artifacts.

Likely fixed by the lent closure fix behind the
`lent_closure_in_a_cell_keeps_its_captures` regression. A closure passed as
`&mut cl` from a capture cell lost its moved captures on the way back, which is
this same `&mut F` helper shape. Replay these seeds from their run artifacts
and delete this entry when they agree.

### `toml::from_str` into a derived struct accepts a missing required field

Real Rust returns an error. The interpreter returns `Ok` with a broken value,
and the script then panics on the first field read. `serde_json::from_str` with
the same structs is right, so the fault is in the toml bridge, not in the
`Deserialize` derive. Found in `shared/src/release.rs` of `hilen/build`, where a
`hilen.toml` with no `[release]` table panics in place of a clean error.

```rust
#!/usr/bin/env rust

use serde::Deserialize;

#[derive(Deserialize)]
struct Outer {
    inner: Inner,
}

#[derive(Deserialize)]
struct Inner {
    value: String,
}

fn main() {
    let parsed: Result<Outer, toml::de::Error> = toml::from_str("other = 1");
    match parsed {
        Ok(outer) => println!("ok {}", outer.inner.value),
        Err(e) => println!("err {e}"),
    }
}
```

Compiled, `rust FILE.rs cmp`:

```
err TOML parse error at line 1, column 1
  |
1 | other = 1
  | ^
missing field `inner`
```

Interpreted, rustscript 0.6.36:

```
thread 'main' panicked at zz_repro.rs:18:40:
cannot read a field of enum
```

The script needs `serde` with `derive` and `toml` in the nearest `Cargo.toml`.

Likely place: `Vm::struct_from_map` in `json_bridge.rs`, the coercion toml and
yaml go through. A missing key with no `#[serde(default)]` becomes `None`
there. The json path checks the same case in `fill_missing`.

### `serde_json::Value` prints like a Rust map, not like json

```rust
fn main() {
    let v: serde_json::Value =
        serde_json::from_str(r#"{"a":[1,2.5,null,true,"s"],"b":{"c":-1}}"#).unwrap();
    println!("{v}");
    println!("{v:?}");
    println!("{}", v["a"][4]);
}
```

Compiled:

```
{"a":[1,2.5,null,true,"s"],"b":{"c":-1}}
Object {"a": Array [Number(1), Number(2.5), Null, Bool(true), String("s")], "b": Object {"c": Number(-1)}}
"s"
```

Interpreted:

```
{"a": [1, 2.5, None, true, "s"], "b": {"c": -1}}
{"a": [1, 2.5, None, true, "s"], "b": {"c": -1}}
s
```

`{:#}` should pretty print like `to_string_pretty`. A json value is a plain
map, list or string at runtime, so the formatter can't tell it apart. Likely
fix: the compiler knows `Ty::Json` at the format site and can flag the argument,
then `bridge/template.rs` renders it through `pvalue_to_json` and the real
`serde_json::Value` formatting.

### Map `extend` and `values_mut` fail at runtime

```rust
use std::collections::HashMap;

fn main() {
    let mut m: HashMap<&str, i32> = HashMap::from([("a", 1)]);
    m.extend([("b", 2)]);
    for v in m.values_mut() {
        *v += 1;
    }
    println!("{} {:?}", m.len(), m.get("a"));
}
```

Compiled prints `2 Some(2)`. Interpreted stops at `extend` with
`unknown method extend on HashMap`, and without it `values_mut` panics with
`assignment through a non-reference value`. Both apply to `BTreeMap` too.
`extend` passes the coverage check because `Vec` has it, so the script starts
and dies halfway. `retain` on a map is missing as well, the coverage check does
report that one. Likely place: `map_methods.rs`, `values_mut` needs element
references like `ValueRef::map_entry`, and the coverage tables in
`bridge_tables_build.rs` should tag `extend` per receiver.

### `Duration` seconds past `i64::MAX` are clamped

```rust
use std::time::Duration;

fn main() {
    println!("{}", Duration::new(u64::MAX, 0).as_secs());
}
```

Compiled prints `18446744073709551615`, interpreted prints
`9223372036854775807`. The bridge keeps `secs` as `Value::Int`, see
`bridge/path_calls.rs`, it should be a `u64` `IntW`.

### Missing std paths found while testing

`Duration::from_secs_f64` and `std::hint::black_box` are not bridged. Both are
refused before the script runs, so nothing runs wrong.

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
