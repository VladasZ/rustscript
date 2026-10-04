//! Strings the way scripts build them. `format!` with positional, named and inline arguments
//! and nested specs, `write!` into a `String`, `+` and `+=`, the conversions to `String`, and
//! `parse` with `?` and `map_err`.

use super::{Exact, FStr, FUSize, Float, Method, SmallUsize, Str, m};

pub(super) const ROWS: &[Method] = &[
    m(
        "fmt_positional",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_d = {0}; format!(\"{{1}}-{{0}}-{{1:?}}\", diff_r, diff_d) }})",
    ),
    m(
        "fmt_named",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_d = {0}; format!(\"{{b}}|{{a:>6}}|{{b:?}}\", a = diff_r, b = diff_d) }})",
    ),
    m(
        "fmt_inline",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_d = {0}; format!(\"{{diff_r}}:{{diff_d:?}}:{{diff_r:<4}}|\") }})",
    ),
    m(
        "fmt_nested_width",
        Str,
        &[SmallUsize],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_w = {0}; format!(\"{{0:>1$}}|{{0:<w$.2}}|{{0:^diff_w$}}|\", diff_r, diff_w, w = diff_w) }})",
    ),
    m(
        "fmt_precision_star",
        Float,
        &[SmallUsize],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_p = {0}; format!(\"{{:.*}}|{{:9.*}}|{{:+.diff_p$e}}\", diff_p, diff_r, diff_p, diff_r, diff_r) }})",
    ),
    m(
        "fmt_write_string",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ use std::fmt::Write as _; let mut diff_s = {r}; let diff_d = {0}; write!(diff_s, \"{{}}:{{:?}}\", diff_d.len(), diff_d).unwrap(); writeln!(diff_s, \"|{{diff_d}}\").unwrap(); writeln!(diff_s).unwrap(); diff_s }})",
    ),
    m(
        "str_plus",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r + \"-\" + &diff_d + diff_d.as_str() }})",
    ),
    m(
        "str_plus_assign",
        Str,
        &[Exact(FStr)],
        Exact(FStr),
        "({{ let mut diff_r = {r}; let diff_d = {0}; diff_r += \"+\"; diff_r += &diff_d; diff_r }})",
    ),
    m(
        "str_conversions",
        Str,
        &[],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_a = String::from(diff_r.as_str()); let diff_b: String = diff_r.as_str().into(); let diff_c = diff_r.to_string(); diff_a + &diff_b + &diff_c + &diff_r.to_owned() }})",
    ),
    m(
        "str_bytes_lower_count",
        Str,
        &[],
        Exact(FUSize),
        "{r}.bytes().filter(|b| b.is_ascii_lowercase()).count()",
    ),
    m(
        "str_bytes_rev",
        Str,
        &[],
        Exact(FStr),
        "{r}.bytes().rev().map(|b| b as char).collect::<String>()",
    ),
    m(
        "str_lines_trim",
        Str,
        &[],
        Exact(FStr),
        "{r}.lines().map(str::trim).map(String::from).collect::<Vec<String>>().join(\"|\")",
    ),
    m(
        "str_split_trim",
        Str,
        &[],
        Exact(FStr),
        "{r}.split(',').map(str::trim).map(str::to_owned).collect::<Vec<String>>().concat()",
    ),
    m(
        "str_parse_question",
        Str,
        &[],
        Exact(FStr),
        "({{ let diff_r = {r}; let diff_f = |s: &str| -> Result<i32, std::num::ParseIntError> {{ let n = s.trim().parse::<i32>()?; Ok(n.wrapping_add(1)) }}; format!(\"{{:?}}\", diff_f(diff_r.as_str())) }})",
    ),
    m(
        "str_parse_map_err",
        Str,
        &[],
        Exact(FStr),
        "({{ let diff_r = {r}; format!(\"{{:?}}\", diff_r.parse::<u8>().map_err(|e| e.to_string())) }})",
    ),
    m(
        "str_parse_scalars",
        Str,
        &[],
        Exact(FStr),
        "({{ let diff_r = {r}; format!(\"{{:?}} {{:?}} {{:?}}\", diff_r.parse::<f64>().map_err(|e| e.to_string()), diff_r.parse::<bool>().map_err(|e| e.to_string()), diff_r.parse::<char>().map_err(|e| e.to_string())) }})",
    ),
];
