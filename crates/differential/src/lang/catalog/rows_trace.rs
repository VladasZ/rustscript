//! Evaluation order tracers. Each prints the value it is handed and passes it on, so wherever
//! the solver puts one, a call argument, an operand, a struct field, an index or the value of
//! an assignment, the order the parts run in is a line of output.

use super::{Char, Float, Int, Method, OrdElem, RecvClass, Same, Str, VecRecv, m, with_elem};

pub(super) const ROWS: &[Method] = &[
    m(
        "eval_trace_int",
        Int,
        &[],
        Same,
        "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
    ),
    m(
        "eval_trace_float",
        Float,
        &[],
        Same,
        "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
    ),
    m(
        "eval_trace_bool",
        RecvClass::Bool,
        &[],
        Same,
        "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
    ),
    m(
        "eval_trace_char",
        Char,
        &[],
        Same,
        "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
    ),
    m(
        "eval_trace_str",
        Str,
        &[],
        Same,
        "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
    ),
    with_elem(
        m(
            "eval_trace_vec",
            VecRecv,
            &[],
            Same,
            "({{ let diff_t = {r}; println!(\"diff eval {{:?}}\", diff_t); diff_t }})",
        ),
        OrdElem,
    ),
];
