//! Rows that hand a closure to `Option`, `Result` and iterator methods, and the mutating methods
//! that need a `let` to act on. Real scripts call these all the time and no other row reaches
//! them.
//!
//! An argument is bound by a `let` after the receiver and the closure reads that binding. An
//! argument written inside the closure would not compile when it holds a `?`, and the `let`
//! keeps the order the ownership checker replays, receiver first.

use super::{
    CopyElem, ELEM, Elem, ErrT, Exact, FBool, FStr, FUSize, Method, Num, OK_PAT, OkT, Opt, OrdElem,
    Res, SAME, STR_PAT, Same, Str, StrElem, TyPat, USIZE_PAT, VecRecv, m, with_elem,
};

pub(super) const ROWS: &[Method] = &[
    // Option
    m(
        "opt_unwrap_or_else",
        Opt,
        &[Elem],
        Elem,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.unwrap_or_else(|| diff_d) }})",
    ),
    m(
        "opt_map_or",
        Opt,
        &[Elem],
        Elem,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.map_or(diff_d, |x| x) }})",
    ),
    m(
        "opt_map_or_else",
        Opt,
        &[Elem],
        Elem,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.map_or_else(|| diff_d, |x| x) }})",
    ),
    m(
        "opt_or_else",
        Opt,
        &[Same],
        Same,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.or_else(|| diff_d) }})",
    ),
    m(
        "opt_ok_or_else",
        Opt,
        &[Exact(FStr)],
        TyPat::Res(ELEM, STR_PAT),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.ok_or_else(|| diff_d) }})",
    ),
    m(
        "opt_is_some_and",
        Opt,
        &[Exact(FBool)],
        Exact(FBool),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.is_some_and(|_| diff_d) }})",
    ),
    m(
        "opt_is_none_or",
        Opt,
        &[Exact(FBool)],
        Exact(FBool),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.is_none_or(|_| diff_d) }})",
    ),
    m(
        "opt_expect",
        Opt,
        &[],
        Elem,
        "{r}.expect(\"differential expect\")",
    ),
    m("opt_as_ref", Opt, &[], Same, "{r}.as_ref().cloned()"),
    m(
        "opt_as_mut",
        Opt,
        &[Elem],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {0}; if let Some(diff_x) = diff_r.as_mut() {{ *diff_x = diff_d; }} diff_r }})",
    ),
    m(
        "opt_get_or_insert",
        Opt,
        &[Elem],
        Elem,
        "({{ let mut diff_r = {r}; let diff_d = {0}; diff_r.get_or_insert(diff_d).clone() }})",
    ),
    m(
        "opt_get_or_insert_with",
        Opt,
        &[Elem],
        Elem,
        "({{ let mut diff_r = {r}; let diff_d = {0}; diff_r.get_or_insert_with(|| diff_d).clone() }})",
    ),
    with_elem(
        m(
            "opt_as_deref",
            Opt,
            &[],
            Exact(FUSize),
            "{r}.as_deref().map_or(0, |s| s.len())",
        ),
        StrElem,
    ),
    with_elem(
        m(
            "opt_as_deref_mut",
            Opt,
            &[],
            Same,
            "({{ let mut diff_r = {r}; if let Some(diff_s) = diff_r.as_deref_mut() {{ diff_s.make_ascii_uppercase(); }} diff_r }})",
        ),
        StrElem,
    ),
    // Result
    m(
        "res_unwrap_or_else",
        Res,
        &[OkT],
        OkT,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.unwrap_or_else(|_| diff_d) }})",
    ),
    m(
        "res_map_or",
        Res,
        &[OkT],
        OkT,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.map_or(diff_d, |x| x) }})",
    ),
    m(
        "res_map_or_else",
        Res,
        &[OkT],
        OkT,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.map_or_else(|_| diff_d, |x| x) }})",
    ),
    m(
        "res_is_ok_and",
        Res,
        &[Exact(FBool)],
        Exact(FBool),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.is_ok_and(|_| diff_d) }})",
    ),
    m(
        "res_is_err_and",
        Res,
        &[Exact(FBool)],
        Exact(FBool),
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.is_err_and(|_| diff_d) }})",
    ),
    m(
        "res_expect",
        Res,
        &[],
        OkT,
        "{r}.expect(\"differential expect\")",
    ),
    m(
        "res_as_ref",
        Res,
        &[],
        TyPat::Opt(OK_PAT),
        "{r}.as_ref().ok().cloned()",
    ),
    m(
        "res_as_mut",
        Res,
        &[OkT],
        TyPat::Opt(OK_PAT),
        "({{ let mut diff_r = {r}; let diff_d = {0}; if let Ok(diff_x) = diff_r.as_mut() {{ *diff_x = diff_d; }} diff_r.ok() }})",
    ),
    m(
        "res_err_as_ref",
        Res,
        &[],
        TyPat::Opt(&ErrT),
        "{r}.as_ref().err().cloned()",
    ),
    // iterator adapters over a `Vec`
    m(
        "iter_chain",
        VecRecv,
        &[Same],
        Same,
        "({{ let diff_r = {r}; let diff_d = {0}; diff_r.into_iter().chain(diff_d).collect::<Vec<{E}>>() }})",
    ),
    m(
        "iter_filter_map",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().filter_map(|(i, x)| if i % 2 == 0 {{ Some(x) }} else {{ None }}).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_find_map",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.into_iter().enumerate().find_map(|(i, x)| if i == 1 {{ Some(x) }} else {{ None }})",
    ),
    m(
        "iter_for_each",
        VecRecv,
        &[],
        Exact(FUSize),
        "({{ let mut diff_n = 0usize; {r}.iter().for_each(|_| diff_n += 1); diff_n }})",
    ),
    m(
        "iter_inspect",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_n = 0usize; let mut diff_v = {r}.into_iter().inspect(|_| diff_n += 1).collect::<Vec<{E}>>(); diff_v.truncate(diff_n); diff_v }})",
    ),
    m(
        "iter_peekable",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_it = {r}.into_iter().peekable(); let diff_skip = diff_it.peek().is_some(); diff_it.skip(usize::from(diff_skip)).collect::<Vec<{E}>>() }})",
    ),
    m(
        "iter_by_ref",
        VecRecv,
        &[],
        TyPat::Tuple2(SAME, SAME),
        "({{ let mut diff_it = {r}.into_iter(); let diff_head = diff_it.by_ref().take(1).collect::<Vec<{E}>>(); (diff_head, diff_it.collect::<Vec<{E}>>()) }})",
    ),
    m(
        "iter_rposition",
        VecRecv,
        &[],
        TyPat::Opt(USIZE_PAT),
        "{r}.iter().enumerate().rposition(|(i, _)| i % 2 == 0)",
    ),
    m(
        "iter_skip_while",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().skip_while(|(i, _)| *i < 1).map(|(_, x)| x).collect::<Vec<{E}>>()",
    ),
    m(
        "iter_take_while",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().enumerate().take_while(|(i, _)| *i < 2).map(|(_, x)| x).collect::<Vec<{E}>>()",
    ),
    with_elem(
        m(
            "iter_cmp",
            VecRecv,
            &[Same],
            Exact(FStr),
            "({{ let diff_r = {r}; let diff_d = {0}; format!(\"{{:?}}\", diff_r.iter().cmp(diff_d.iter())) }})",
        ),
        OrdElem,
    ),
    // a float item makes the `None` of a NaN reachable
    with_elem(
        m(
            "iter_partial_cmp",
            VecRecv,
            &[Same],
            Exact(FStr),
            "({{ let diff_r = {r}; let diff_d = {0}; format!(\"{{:?}}\", diff_r.iter().partial_cmp(diff_d.iter())) }})",
        ),
        Num,
    ),
    // `Vec` methods that write in place
    m(
        "vec_append",
        VecRecv,
        &[Same],
        Same,
        "({{ let mut diff_r = {r}; let mut diff_d = {0}; diff_r.append(&mut diff_d); diff_r.truncate(diff_r.len() - diff_d.len()); diff_r }})",
    ),
    m(
        "vec_extend_from_slice",
        VecRecv,
        &[Same],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {0}; diff_r.extend_from_slice(&diff_d); diff_r }})",
    ),
    m(
        "vec_first_mut",
        VecRecv,
        &[Elem],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {0}; if let Some(diff_x) = diff_r.first_mut() {{ *diff_x = diff_d; }} diff_r }})",
    ),
    with_elem(
        m(
            "vec_sort_unstable",
            VecRecv,
            &[],
            Same,
            "({{ let mut diff_r = {r}; diff_r.sort_unstable(); diff_r }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_copy_from_slice",
            VecRecv,
            &[Same],
            Same,
            "({{ let mut diff_r = {r}; let diff_d = {0}; let diff_n = diff_r.len().min(diff_d.len()); diff_r[..diff_n].copy_from_slice(&diff_d[..diff_n]); diff_r }})",
        ),
        CopyElem,
    ),
    // every written item is a clone, and the item it replaces drops right there
    m(
        "vec_clone_from_slice",
        VecRecv,
        &[Same],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {0}; let diff_n = diff_r.len().min(diff_d.len()); diff_r[..diff_n].clone_from_slice(&diff_d[..diff_n]); diff_r }})",
    ),
    // the length is part of the type, a slice of another length is `None`
    m(
        "vec_as_array",
        VecRecv,
        &[],
        TyPat::Opt(USIZE_PAT),
        "{r}.as_array::<2>().map(|a| a.len())",
    ),
    m(
        "str_into_string",
        Str,
        &[],
        Exact(FStr),
        "({{ let diff_b: Box<str> = {r}.into(); diff_b.into_string() }})",
    ),
];
