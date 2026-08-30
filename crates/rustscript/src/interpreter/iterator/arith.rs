//! `sum` and `product` over numbers, shared by the lazy iterator and the eager vec path so both
//! agree on every width.

use num_traits::AsPrimitive;

use anyhow::{Result, anyhow, bail};

use crate::interpreter::bytecode::ScalarTy;
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::value::Value;

/// The float side of a reduction. Runs in `f32` when the turbofish or the first float element
/// says so, because every step must round the way the native `f32` does, a `f32::MAX + f32::MAX`
/// must overflow to `inf` before the next element joins.
enum FloatAcc {
    F64(f64),
    F32(f32),
}

impl FloatAcc {
    fn new(target: Option<&ScalarTy>, identity: f64) -> Self {
        match target {
            Some(ScalarTy::F32) => FloatAcc::F32(AsPrimitive::<f32>::as_(identity)),
            _ => FloatAcc::F64(identity),
        }
    }

    fn apply(
        &mut self,
        value: &Value,
        op: fn(f64, f64) -> f64,
        op32: fn(f32, f32) -> f32,
    ) -> Result<()> {
        match (&mut *self, value) {
            (FloatAcc::F32(acc), Value::F32(x)) => *acc = op32(*acc, *x),
            (FloatAcc::F32(acc), Value::Float(x)) => *acc = op32(*acc, AsPrimitive::<f32>::as_(*x)),
            (FloatAcc::F64(acc), Value::Float(x)) => *acc = op(*acc, *x),
            // the elements decide the width when the turbofish is missing
            (FloatAcc::F64(acc), Value::F32(x)) => {
                *self = FloatAcc::F32(op32(AsPrimitive::<f32>::as_(*acc), *x));
            }
            (_, other) => bail!("reduction needs numbers, got {}", other.type_name()),
        }
        Ok(())
    }

    fn finish(
        self,
        op: fn(f64, f64) -> f64,
        op32: fn(f32, f32) -> f32,
        integers: Option<i128>,
    ) -> Value {
        match (self, integers) {
            (FloatAcc::F32(acc), None) => Value::F32(acc),
            (FloatAcc::F32(acc), Some(i)) => Value::F32(op32(acc, AsPrimitive::<f32>::as_(i))),
            (FloatAcc::F64(acc), None) => Value::Float(acc),
            (FloatAcc::F64(acc), Some(i)) => Value::Float(op(acc, AsPrimitive::<f64>::as_(i))),
        }
    }
}

struct IntAcc {
    value: i128,
    low: i128,
    high: i128,
    bounded: bool,
    seen_width: Option<IntWidth>,
    seen: bool,
}

impl IntAcc {
    fn new(target: Option<&ScalarTy>, identity: i128) -> Self {
        // without a target the width of the first tagged element is the width of the result
        let (low, high) = match target {
            Some(ScalarTy::Int(width)) => (width.min(), width.max()),
            _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
        };
        IntAcc {
            value: identity,
            low,
            high,
            bounded: matches!(target, Some(ScalarTy::Int(_))),
            seen_width: None,
            seen: false,
        }
    }

    fn apply(
        &mut self,
        value: i128,
        width: IntWidth,
        op: fn(i128, i128) -> Option<i128>,
        overflow: &str,
    ) -> Result<()> {
        if !self.bounded {
            (self.low, self.high) = (width.min(), width.max());
            self.bounded = true;
            self.seen_width = Some(width);
        }
        self.seen = true;
        self.value = op(self.value, value).ok_or_else(|| anyhow!("{overflow}"))?;
        // a `sum::<u8>()` overflows at 255
        if self.value < self.low || self.value > self.high {
            bail!("{overflow}");
        }
        Ok(())
    }

    fn finish(self, target: Option<&ScalarTy>, what: &str) -> Value {
        if let Some(ScalarTy::Int(width)) = target {
            // keep the tag, otherwise `!0u16` gives -1
            Value::int_of_width(self.value, *width)
        } else if let Some(width) = self.seen_width {
            Value::int_of_width(self.value, width)
        } else {
            Value::Int(
                i64::try_from(self.value)
                    .unwrap_or_else(|_| panic!("{what} is range-checked per step")),
            )
        }
    }
}

#[derive(Clone, Copy)]
struct Reduction {
    op: fn(i128, i128) -> Option<i128>,
    op64: fn(f64, f64) -> f64,
    op32: fn(f32, f32) -> f32,
    overflow: &'static str,
    what: &'static str,
}

const SUM: Reduction = Reduction {
    op: i128::checked_add,
    op64: |a, b| a + b,
    op32: |a, b| a + b,
    overflow: "attempt to add with overflow",
    what: "sum",
};

const PRODUCT: Reduction = Reduction {
    op: i128::checked_mul,
    op64: |a, b| a * b,
    op32: |a, b| a * b,
    overflow: "attempt to multiply with overflow",
    what: "product",
};

/// Folds one element at a time. A lazy source is pulled through this, so an accumulator
/// overflow panics before the next element is produced, the way the native `Sum` and `Product`
/// impls do. Draining the source first would report a panic from a later element instead.
pub(in crate::interpreter) struct Reducer<'a> {
    target: Option<&'a ScalarTy>,
    reduction: Reduction,
    integers: IntAcc,
    floats: FloatAcc,
    has_float: bool,
}

impl<'a> Reducer<'a> {
    fn new(
        target: Option<&'a ScalarTy>,
        reduction: Reduction,
        int_identity: i128,
        float_identity: f64,
    ) -> Self {
        Self {
            target,
            reduction,
            // i128 so a `u64` element past `i64::MAX` keeps its value
            integers: IntAcc::new(target, int_identity),
            floats: FloatAcc::new(target, float_identity),
            has_float: false,
        }
    }

    pub(in crate::interpreter) fn push(&mut self, value: &Value) -> Result<()> {
        let r = self.reduction;
        if let Some((int, width)) = value.int_parts() {
            return self.integers.apply(int, width, r.op, r.overflow);
        }
        self.floats.apply(value, r.op64, r.op32)?;
        self.has_float = true;
        Ok(())
    }

    pub(in crate::interpreter) fn finish(self) -> Value {
        let r = self.reduction;
        // only a `sum::<f64>()` turbofish tells an empty float sum from an integer one
        let float_target = matches!(self.target, Some(ScalarTy::F32 | ScalarTy::F64));
        if self.has_float || (float_target && !self.integers.seen) {
            // the integer side only joins with a value, so it can't cancel the -0.0 identity
            let joined = self.integers.seen.then_some(self.integers.value);
            self.floats.finish(r.op64, r.op32, joined)
        } else {
            self.integers.finish(self.target, r.what)
        }
    }
}

/// `Sum` for floats starts at -0.0, so negative zeros keep the sign
pub(in crate::interpreter) fn sum_reducer(target: Option<&ScalarTy>) -> Reducer<'_> {
    Reducer::new(target, SUM, 0, -0.0)
}

pub(in crate::interpreter) fn product_reducer(target: Option<&ScalarTy>) -> Reducer<'_> {
    Reducer::new(target, PRODUCT, 1, 1.0)
}

fn reduce(items: impl IntoIterator<Item = Value>, mut reducer: Reducer<'_>) -> Result<Value> {
    for value in items {
        reducer.push(&value)?;
    }
    Ok(reducer.finish())
}

pub(in crate::interpreter) fn sum_values(
    items: impl IntoIterator<Item = Value>,
    target: Option<&ScalarTy>,
) -> Result<Value> {
    reduce(items, sum_reducer(target))
}

pub(in crate::interpreter) fn product_values(
    items: impl IntoIterator<Item = Value>,
    target: Option<&ScalarTy>,
) -> Result<Value> {
    reduce(items, product_reducer(target))
}
