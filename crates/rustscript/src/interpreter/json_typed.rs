//! The typed shapes of a planned parse. Each one tells the format what it reads, so a value of
//! another kind is the error the real serde type gives, `invalid type: null, expected struct
//! Config`, with the position the format adds.

use std::fmt::{Formatter, Result as FmtResult};
use std::sync::Arc;

use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Unexpected, Visitor};

use super::json_bridge::{JsonPlan, ParseCx, PlanSeed, PlanVisitor, StructPlan};
use super::resolver::bare;
use super::value::Value;

/// `Option<T>`. A null is `None`, anything else is `Some` of what the inner plan reads, so an
/// option inside an option or inside a vec keeps every level.
pub(super) struct OptVisitor<'a> {
    pub(super) inner: &'a JsonPlan,
    pub(super) cx: &'a ParseCx<'a>,
}

impl<'de> Visitor<'de> for OptVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut Formatter) -> FmtResult {
        f.write_str("option")
    }

    fn visit_none<E: Error>(self) -> Result<Value, E> {
        Ok(Value::none())
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::none())
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        let inner = PlanSeed {
            plan: self.inner,
            cx: self.cx,
        }
        .deserialize(d)?;
        Ok(Value::some(inner))
    }
}

/// A struct, a sequence or a map. It takes only the kinds the real type takes.
pub(super) struct TypedVisitor<'a> {
    pub(super) plan: &'a JsonPlan,
    pub(super) cx: &'a ParseCx<'a>,
}

impl TypedVisitor<'_> {
    fn untyped(&self) -> PlanVisitor<'_> {
        PlanVisitor {
            plan: self.plan,
            cx: self.cx,
        }
    }
}

impl<'de> Visitor<'de> for TypedVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut Formatter) -> FmtResult {
        match self.plan {
            JsonPlan::Struct(sp) => write!(f, "struct {}", bare(&sp.info().shape.name)),
            JsonPlan::Map(..) => f.write_str("a map"),
            _ => f.write_str("a sequence"),
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Value, A::Error> {
        match self.plan {
            JsonPlan::Struct(sp) => struct_from_seq(sp, self.cx, seq),
            JsonPlan::Map(..) => Err(A::Error::invalid_type(Unexpected::Seq, &self)),
            _ => self.untyped().visit_seq(seq),
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Value, A::Error> {
        match self.plan {
            JsonPlan::Struct(_) | JsonPlan::Map(..) => self.untyped().visit_map(map),
            _ => Err(A::Error::invalid_type(Unexpected::Map, &self)),
        }
    }
}

/// A derived struct also reads from a sequence, the fields in declaration order.
fn struct_from_seq<'de, A: SeqAccess<'de>>(
    sp: &Arc<StructPlan>,
    cx: &ParseCx<'_>,
    mut seq: A,
) -> Result<Value, A::Error> {
    let info = sp.info();
    let count = sp.fields().len();
    let mut values = Vec::with_capacity(count);
    for (slot, plan) in sp.fields().iter().enumerate() {
        if let Some(value) = seq.next_element_seed(PlanSeed { plan, cx })? {
            values.push(sp.field_value(slot, value));
            continue;
        }
        // a short sequence fills a `#[serde(default)]` field and fails on any other
        let default = info.defaults.get(slot).and_then(Option::as_ref);
        let (Some(chunk), Some(vm)) = (default, cx.vm) else {
            let plural = if count == 1 { "" } else { "s" };
            let expected = format!(
                "struct {} with {count} element{plural}",
                bare(&info.shape.name)
            );
            return Err(A::Error::invalid_length(slot, &expected.as_str()));
        };
        values.push(
            vm.run_chunk(chunk, &[], &[], false)
                .map_err(|e| A::Error::custom(format!("{e:#}")))?,
        );
    }
    Ok(Value::structure(info.shape.clone(), values))
}
