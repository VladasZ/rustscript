//! A structural case for slice patterns with a `..` rest: bare and named,
//! at the front, middle, and back, behind a guard, over a probe function
//! taking `&[i64]`. The interpreter's rest handling once missed `rest @ ..`
//! entirely and bound tails in reversed order, and nothing generated ever
//! exercised the family, which is why this case exists.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::typed::GeneratedExpr;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SliceCase {
    pub id: usize,
    pub values: Vec<i64>,
    pub guard_length: usize,
    pub empty_arm: GeneratedExpr,
    pub single_arm: GeneratedExpr,
    pub front_arm: GeneratedExpr,
    pub bookend_arm: GeneratedExpr,
    pub pair_arm: GeneratedExpr,
    pub tail_arm: GeneratedExpr,
    pub ends_empty_arm: GeneratedExpr,
}

impl SliceCase {
    /// Two probe functions per case. Match ergonomics bind the elements as
    /// references, so each arm rebinds its scalars by deref before the
    /// generated expression runs on plain `i64` values.
    pub(crate) fn prelude(&self) -> String {
        format!(
            r#"fn generated_slice_probe_{id}(values: &[i64]) -> String {{
    match values {{
        [] => {empty},
        [slice_only_{id}] => {{
            let slice_only_{id} = *slice_only_{id};
            {single}
        }}
        [slice_first_{id}, slice_rest_{id} @ ..] if slice_rest_{id}.len() > {guard}usize => {{
            let slice_first_{id} = *slice_first_{id};
            format!("{{}}|{{:?}}|{{}}", {front}, slice_rest_{id}, slice_rest_{id}.len())
        }}
        [slice_first_{id}, .., slice_penult_{id}, slice_last_{id}] => {{
            let slice_first_{id} = *slice_first_{id};
            let slice_penult_{id} = *slice_penult_{id};
            let slice_last_{id} = *slice_last_{id};
            {bookend}
        }}
        [slice_first_{id}, slice_last_{id}] => {{
            let slice_first_{id} = *slice_first_{id};
            let slice_last_{id} = *slice_last_{id};
            {pair}
        }}
    }}
}}

fn generated_slice_ends_{id}(values: &[i64]) -> String {{
    match values {{
        [slice_head_{id} @ .., slice_tail_{id}] => {{
            let slice_tail_{id} = *slice_tail_{id};
            format!("{{:?}}+{{}}", slice_head_{id}, {tail})
        }}
        [] => {ends_empty},
    }}
}}

"#,
            id = self.id,
            guard = self.guard_length,
            empty = self.empty_arm.render(),
            single = self.single_arm.render(),
            front = self.front_arm.render(),
            bookend = self.bookend_arm.render(),
            pair = self.pair_arm.render(),
            tail = self.tail_arm.render(),
            ends_empty = self.ends_empty_arm.render()
        )
    }

    pub(crate) fn render(&self) -> String {
        let values = self
            .values
            .iter()
            .map(|value| format!("{value}i64"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"    {{
        let generated_slice_values_{id}: Vec<i64> = vec![{values}];
        let generated_slice_output_{id} = generated_slice_probe_{id}(&generated_slice_values_{id});
        let generated_slice_ends_output_{id} = generated_slice_ends_{id}(&generated_slice_values_{id});
        println!("generated-slice-{id}:{{generated_slice_output_{id}}}|{{generated_slice_ends_output_{id}}}");
    }}
"#,
            id = self.id,
            values = values
        )
    }

    pub(crate) fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if !self.values.is_empty() || self.guard_length != 0 {
            let mut candidate = self.clone();
            candidate.values.clear();
            candidate.guard_length = 0;
            candidates.push(candidate);
        }
        if self.values.len() > 1 {
            let mut candidate = self.clone();
            candidate.values.truncate(self.values.len() - 1);
            candidates.push(candidate);
        }
        candidates
    }

    fn expressions(&self) -> [&GeneratedExpr; 7] {
        [
            &self.empty_arm,
            &self.single_arm,
            &self.front_arm,
            &self.bookend_arm,
            &self.pair_arm,
            &self.tail_arm,
            &self.ends_empty_arm,
        ]
    }

    pub(crate) fn shape(&self, output: &mut String) {
        output.push_str("slice-match:");
        for expression in self.expressions() {
            expression.shape(output);
        }
    }

    pub(crate) fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("slice-pattern");
        output.insert("slice-rest-pattern");
        output.insert("match-guard");
        for expression in self.expressions() {
            expression.features(output);
        }
    }
}
