//! `BTreeMap`, `BTreeSet` and `VecDeque` built from a `Vec`, and the `Vec` methods that sort,
//! cut and splice in place. The ordered collections print directly, they keep one order.

use super::{
    ELEM, Elem, Exact, FBool, FStr, KeyElem, Method, OrdElem, OrdSilent, RecvClass, Same,
    SmallUsize, TyPat, VecRecv, m, with_elem,
};

pub(super) const ROWS: &[Method] = &[
    with_elem(
        m(
            "vec_btree_set",
            VecRecv,
            &[],
            Same,
            "{r}.into_iter().collect::<std::collections::BTreeSet<{E}>>().into_iter().collect::<Vec<{E}>>()",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_set_debug",
            VecRecv,
            &[],
            Exact(FStr),
            "format!(\"{{:?}}\", {r}.iter().collect::<std::collections::BTreeSet<_>>())",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_set_pop",
            VecRecv,
            &[],
            TyPat::Opt(ELEM),
            "({{ let mut diff_s = {r}.into_iter().collect::<std::collections::BTreeSet<{E}>>(); diff_s.pop_first(); diff_s.pop_last() }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_set_range",
            VecRecv,
            &[Elem],
            Same,
            "({{ let diff_s = {r}.into_iter().collect::<std::collections::BTreeSet<{E}>>(); let diff_d = {0}; diff_s.range(diff_d..).rev().cloned().collect::<Vec<{E}>>() }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_map_counts",
            VecRecv,
            &[],
            Exact(FStr),
            "({{ let mut diff_m = std::collections::BTreeMap::new(); for diff_x in {r} {{ *diff_m.entry(diff_x).or_insert(0usize) += 1; }} format!(\"{{:?}}\", diff_m) }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_map_and_modify",
            VecRecv,
            &[],
            Exact(FStr),
            "({{ let mut diff_m = std::collections::BTreeMap::new(); for diff_x in {r} {{ diff_m.entry(diff_x).and_modify(|n| *n += 2).or_insert(1usize); }} format!(\"{{:?}} {{:?}}\", diff_m.first_key_value(), diff_m.last_key_value()) }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_btree_map_enumerate",
            VecRecv,
            &[],
            Exact(FStr),
            "format!(\"{{:?}}\", {r}.into_iter().enumerate().map(|(i, x)| (i % 3, x)).collect::<std::collections::BTreeMap<usize, {E}>>())",
        ),
        OrdElem,
    ),
    m(
        "vec_deque_rotate",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_q = {r}.into_iter().collect::<std::collections::VecDeque<{E}>>(); if let Some(diff_x) = diff_q.pop_front() {{ diff_q.push_back(diff_x); }} diff_q.into_iter().collect::<Vec<{E}>>() }})",
    ),
    m(
        "vec_deque_push_front",
        VecRecv,
        &[Elem],
        Same,
        "({{ let mut diff_q = std::collections::VecDeque::from({r}); let diff_d = {0}; diff_q.push_front(diff_d); diff_q.pop_back(); diff_q.into_iter().collect::<Vec<{E}>>() }})",
    ),
    with_elem(
        m(
            "vec_deque_debug",
            VecRecv,
            &[],
            Exact(FStr),
            "format!(\"{{:?}}\", {r}.into_iter().rev().collect::<std::collections::VecDeque<{E}>>())",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "map_and_modify",
            RecvClass::Map,
            &[TyPat::Key, TyPat::Val],
            TyPat::Val,
            "({{ let mut diff_r = {r}; let diff_k = {0}; let diff_v = {1}; diff_r.entry(diff_k).and_modify(|v| *v = diff_v.clone()).or_default().clone() }})",
        ),
        KeyElem,
    ),
    // sorts
    with_elem(
        m(
            "vec_sort_by_key_reverse",
            VecRecv,
            &[],
            Same,
            "({{ let mut diff_r = {r}; diff_r.sort_by_key(|x| std::cmp::Reverse(x.clone())); diff_r }})",
        ),
        OrdSilent,
    ),
    with_elem(
        m(
            "vec_sort_by_cmp",
            VecRecv,
            &[],
            Same,
            "({{ let mut diff_r = {r}; diff_r.sort_by(|a, b| b.cmp(a)); diff_r }})",
        ),
        OrdElem,
    ),
    with_elem(
        m(
            "vec_is_sorted",
            VecRecv,
            &[],
            Exact(FBool),
            "{r}.is_sorted()",
        ),
        OrdElem,
    ),
    m(
        "vec_dedup_by_key",
        VecRecv,
        &[],
        Same,
        "({{ let mut diff_r = {r}; let mut diff_i = 0usize; diff_r.dedup_by_key(|_| {{ diff_i += 1; diff_i / 2 }}); diff_r }})",
    ),
    with_elem(
        m(
            "vec_dedup_by",
            VecRecv,
            &[],
            Same,
            "({{ let mut diff_r = {r}; diff_r.dedup_by(|a, b| a == b); diff_r }})",
        ),
        OrdElem,
    ),
    // cuts and splices, each panics past the end
    m(
        "vec_drain_front",
        VecRecv,
        &[SmallUsize],
        Same,
        "({{ let mut diff_r = {r}; let mut diff_d = diff_r.drain(..{0}).collect::<Vec<{E}>>(); diff_d.extend(diff_r); diff_d }})",
    ),
    m(
        "vec_drain_tail",
        VecRecv,
        &[SmallUsize],
        Same,
        "({{ let mut diff_r = {r}; diff_r.drain({0}..).rev().collect::<Vec<{E}>>() }})",
    ),
    m(
        "vec_split_off",
        VecRecv,
        &[SmallUsize],
        Same,
        "({{ let mut diff_r = {r}; let mut diff_t = diff_r.split_off({0}); diff_t.extend(diff_r); diff_t }})",
    ),
    m(
        "vec_insert_at",
        VecRecv,
        &[SmallUsize, Elem],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {1}; diff_r.insert({0}, diff_d); diff_r }})",
    ),
    m(
        "vec_extend_rev",
        VecRecv,
        &[Same],
        Same,
        "({{ let mut diff_r = {r}; let diff_d = {0}; diff_r.extend(diff_d.into_iter().rev()); diff_r }})",
    ),
    m(
        "vec_rotate_left",
        VecRecv,
        &[SmallUsize],
        Same,
        "({{ let mut diff_r = {r}; diff_r.rotate_left({0}); diff_r }})",
    ),
];
