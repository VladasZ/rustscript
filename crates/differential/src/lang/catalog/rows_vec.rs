//! `Vec` rows.

use super::{
    ELEM, Elem, ElemReq, Exact, FBool, FStr, FUSize, KeyElem, Method, Num, Opt, OrdElem, SAME,
    Same, SmallUsize, StrElem, TyPat, USIZE_PAT, VecOfVec, VecRecv, m, with_elem,
};

pub(super) const ROWS: &[Method] = &[
    m(
        "first",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.first().cloned()",
    ),
    m(
        "last",
        VecRecv,
        &[],
        TyPat::Opt(ELEM),
        "{r}.last().cloned()",
    ),
    m(
        "get",
        VecRecv,
        &[SmallUsize],
        TyPat::Opt(ELEM),
        "{r}.get({0}).cloned()",
    ),
    // panics out of bounds
    m(
        "vec_index",
        VecRecv,
        &[SmallUsize],
        Elem,
        "{r}[{0}].clone()",
    ),
    m(
        "vec_contains",
        VecRecv,
        &[Elem],
        Exact(FBool),
        "{r}.contains(&{0})",
    ),
    with_elem(
        m(
            "vec_max",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.iter().max().cloned()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_min",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.iter().min().cloned()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_sum",
            VecRecv,
            &[],
            Elem,
            "{r}.iter().copied().sum::<{E}>()",
        ),
        Num,
    ),
    with_elem(
        m(
            "vec_product",
            VecRecv,
            &[],
            Elem,
            "{r}.iter().copied().product::<{E}>()",
        ),
        Num,
    ),
    with_elem(
        m(
            "vec_sorted",
            VecRecv,
            &[],
            Same,
            "({{ let mut sorted = {r}; sorted.sort(); sorted }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_sorted_dedup",
            VecRecv,
            &[],
            Same,
            "({{ let mut sorted = {r}; sorted.sort(); sorted.dedup(); sorted }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_binary_search",
            VecRecv,
            &[Elem],
            TyPat::Res(USIZE_PAT, USIZE_PAT),
            "({{ let mut sorted = {r}; sorted.sort(); sorted.binary_search(&{0}) }})",
        ),
        OrdElem,
    ),
    m(
        "vec_reversed",
        VecRecv,
        &[],
        Same,
        "{r}.into_iter().rev().collect::<Vec<{E}>>()",
    ),
    m(
        "vec_take",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().take({0}).collect::<Vec<{E}>>()",
    ),
    m(
        "vec_skip",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().skip({0}).collect::<Vec<{E}>>()",
    ),
    // panics on a zero step
    m(
        "vec_step_by",
        VecRecv,
        &[SmallUsize],
        Same,
        "{r}.into_iter().step_by({0}).collect::<Vec<{E}>>()",
    ),
    with_elem(
        m(
            "vec_repeat",
            VecRecv,
            &[SmallUsize],
            Same,
            "{r}.repeat({0})",
        ),
        ElemReq::Copy,
    ),
    m(
        "vec_concat_with",
        VecRecv,
        &[Same],
        Same,
        "[{r}, {0}].concat()",
    ),
    // panics when the split point passes the end
    m(
        "vec_split_at",
        VecRecv,
        &[SmallUsize],
        TyPat::Tuple2(SAME, SAME),
        "({{ let (a, b) = {r}.split_at({0}); (a.to_vec(), b.to_vec()) }})",
    ),
    m(
        "vec_split_first",
        VecRecv,
        &[],
        TyPat::Opt(&TyPat::Tuple2(ELEM, SAME)),
        "{r}.split_first().map(|(a, b)| (a.clone(), b.to_vec()))",
    ),
    m(
        "vec_split_last",
        VecRecv,
        &[],
        TyPat::Opt(&TyPat::Tuple2(ELEM, SAME)),
        "{r}.split_last().map(|(a, b)| (a.clone(), b.to_vec()))",
    ),
    // panics on a zero chunk size
    m(
        "vec_chunk_lens",
        VecRecv,
        &[SmallUsize],
        TyPat::Vec(USIZE_PAT),
        "{r}.chunks({0}).map(<[{E}]>::len).collect::<Vec<usize>>()",
    ),
    m(
        "vec_window_count",
        VecRecv,
        &[SmallUsize],
        Exact(FUSize),
        "{r}.windows({0}).count()",
    ),
    with_elem(
        m(
            "vec_window_sums",
            VecRecv,
            &[],
            Same,
            "{r}.windows(2).map(|w| w[0] + w[1]).collect::<Vec<{E}>>()",
        ),
        Num,
    ),
    m(
        "vec_zip",
        VecRecv,
        &[Same],
        TyPat::Vec(&TyPat::Tuple2(ELEM, ELEM)),
        "{r}.into_iter().zip({0}).collect::<Vec<({E}, {E})>>()",
    ),
    m(
        "vec_enumerate",
        VecRecv,
        &[],
        TyPat::Vec(&TyPat::Tuple2(USIZE_PAT, ELEM)),
        "{r}.into_iter().enumerate().collect::<Vec<(usize, {E})>>()",
    ),
    with_elem(
        m(
            "vec_to_set_max",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "{r}.into_iter().collect::<HashSet<{E}>>().into_iter().max()",
        ),
        KeyElem,
    ),
    with_elem(
        m(
            "vec_join",
            VecRecv,
            &[Exact(FStr)],
            Exact(FStr),
            "{r}.join({0}.as_str())",
        ),
        StrElem,
    ),
    with_elem(
        m("vec_str_concat", VecRecv, &[], Exact(FStr), "{r}.concat()"),
        StrElem,
    ),
    m(
        "vec_concat",
        VecOfVec,
        &[],
        TyPat::Vec(ELEM),
        "{r}.concat()",
    ),
    m(
        "vec_flatten_len",
        VecOfVec,
        &[],
        Exact(FUSize),
        "{r}.into_iter().flatten().count()",
    ),
    // Option
    m("is_some", Opt, &[], Exact(FBool), "{r}.is_some()"),
    m("is_none", Opt, &[], Exact(FBool), "{r}.is_none()"),
];
