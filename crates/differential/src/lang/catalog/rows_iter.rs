//! Iterator chains the way scripts write them, the adapters and terminals no other row reaches,
//! and closures that print as they run.
//!
//! A closure that prints needs items with a stable `Debug`, so those rows take `Ord` items. A
//! float item would print the sign of a NaN, which real Rust leaves open.

use super::{
    ELEM, Elem, Exact, FStr, FUSize, Method, OrdElem, Same, StrElem, TyPat, VecRecv, m, with_elem,
};

pub(super) const ROWS: &[Method] = &[
    m(
        "iter_scan",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().scan(0usize, |n, (i, x)| {{ *n += i; if *n > 3 {{ None }} else {{ Some(x) }} }}).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_scan_running",
        VecRecv,
        &[],
        Same,
        "({{ let diff_r = {r}; let diff_n = diff_r.iter().scan(1usize, |n, _| {{ *n += 3; Some(*n) }}).last().unwrap_or(0); diff_r.into_iter().take(diff_n % 4).collect::<Vec<{E}>>() }})",
    ),
    m(
        "iter_map_while",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().map_while(|(i, x)| if i < 2 {{ Some(x) }} else {{ None }}).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_cycle",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().cycle().take(5).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_flat_map",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().flat_map(|x| [x.clone(), x]).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_flatten_options",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().map(|(i, x)| if i % 2 == 0 {{ Some(x) }} else {{ None }}).flatten().collect::<Vec<{E}>>()",
    ),
    m(
        "iter_zip_unzip",
        VecRecv,
        &[Same],
        Same,
        "({{ let diff_r = {r}; let diff_d = {0}; let (mut diff_a, diff_b) = diff_r.into_iter().zip(diff_d).unzip::<{E}, {E}, Vec<{E}>, Vec<{E}>>(); diff_a.extend(diff_b); diff_a }})",
    ),
    m(
        "iter_partition",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_i = 0usize; let (mut diff_a, diff_b) = {r}.into_iter().partition::<Vec<{E}>, _>(|_| {{ diff_i += 1; diff_i % 2 == 0 }}); diff_a.extend(diff_b); diff_a }})",
    ),
    m(
        "iter_max_by_key",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.into_iter().enumerate().max_by_key(|(i, _)| i % 3).map(|(_, x)| x)",
    ),
    m(
        "iter_min_by_key",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.into_iter().enumerate().min_by_key(|(i, _)| i % 3).map(|(_, x)| x)",
    ),
    with_elem(
        m(
            "iter_max_by",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.into_iter().max_by(|a, b| b.cmp(a))",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_min_by",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.into_iter().min_by(|a, b| b.cmp(a))",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_reduce",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.into_iter().reduce(|a, b| if b > a {{ b }} else {{ a }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_find",
            VecRecv,
            &[Elem],
            TyPat::Opt(ELEM),
            "({{ let diff_r = {r}; let diff_d = {0}; diff_r.into_iter().find(|x| *x >= diff_d) }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_collect_string",
            VecRecv,
            &[],
            Exact(FStr),
            "{r}.iter().map(|s| s.as_str()).collect::<String>()",
        ),
        StrElem,
    ),
    with_elem(
        m(
            "iter_collect_result",
            VecRecv,
            &[],
            Exact(FStr),
            "format!(\"{{:?}}\", {r}.iter().map(|s| s.trim().parse::<i32>()).collect::<Result<Vec<_>, _>>())",
        ),
        StrElem,
    ),
    m(
        "iter_collect_option",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().map(|(i, x)| if i < 3 {{ Some(x) }} else {{ None }}).collect::<Option<Vec<{E}>>>().unwrap_or_default()",
    ),
    m(
        "iter_peek_loop",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_it = {r}.into_iter().peekable(); let mut diff_v = Vec::new(); while let Some(diff_x) = diff_it.next() {{ if diff_it.peek().is_some() {{ diff_v.push(diff_x); }} }} diff_v }})",
    ),
    m(
        "iter_next_if",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_it = {r}.into_iter().enumerate().peekable(); let mut diff_v = Vec::new(); while let Some((_, diff_x)) = diff_it.next_if(|(i, _)| *i < 2) {{ diff_v.push(diff_x); }} diff_v }})",
    ),
    m(
        "iter_stored_next",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "({{ let mut diff_it = {r}.into_iter(); diff_it.next(); diff_it.next() }})",
    ),
    m(
        "iter_chunks_exact",
        VecRecv,
        &[],
        Exact(FUSize),
        "{r}.chunks_exact(2).map(|c| c.len()).sum::<usize>()",
    ),
    m(
        "iter_rchunks",
        VecRecv,
        &[],
        Same,
        "{r}.rchunks(2).map(|c| c.to_vec()).collect::<Vec<Vec<{E}>>>().concat()",
    ),
    m(
        "iter_range_rev_step",
        VecRecv,
        &[],
        Exact(FUSize),
        "(0..{r}.len()).rev().step_by(2).map(|i| i * 3).sum::<usize>()",
    ),
    m(
        "iter_exact_len",
        VecRecv,
        &[],
        Exact(FUSize),
        "({{ let diff_r = {r}; let mut diff_it = diff_r.iter().skip(1).enumerate(); diff_it.next(); diff_it.len() }})",
    ),
    // closures that print as they run, so the order they are called in is part of the output
    with_elem(
        m(
            "iter_inspect_print",
            VecRecv,
            &[],
            Same,
            "{r}.into_iter().inspect(|x| println!(\"diff inspect {{:?}}\", x)).collect::<Vec<{E}>>()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_map_print",
            VecRecv,
            &[],
            Same,
            "{r}.into_iter().map(|x| {{ println!(\"diff map {{:?}}\", x); x }}).rev().collect::<Vec<{E}>>()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_filter_print",
            VecRecv,
            &[],
            Same,
            "({{ let mut diff_i = 0usize; {r}.into_iter().filter(|x| {{ println!(\"diff filter {{:?}}\", x); diff_i += 1; diff_i % 2 == 1 }}).map(|x| {{ println!(\"diff kept {{:?}}\", x); x }}).collect::<Vec<{E}>>() }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_fold_print",
            VecRecv,
            &[],
            Exact(FUSize),
            "{r}.iter().fold(0usize, |n, x| {{ println!(\"diff fold {{}} {{:?}}\", n, x); n + 1 }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "iter_for_each_print",
            VecRecv,
            &[],
            Exact(FUSize),
            "({{ let diff_r = {r}; diff_r.iter().rev().for_each(|x| println!(\"diff each {{:?}}\", x)); diff_r.len() }})",
        ),
        OrdElem,
    ),
];
