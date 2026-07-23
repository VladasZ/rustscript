use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SemanticCase {
    BorrowedVector {
        id: usize,
        values: Vec<i64>,
        delta: i64,
        start: usize,
        take: usize,
    },
    OwnedRecord {
        id: usize,
        label: String,
        values: Vec<i64>,
        extra: i64,
    },
    ResultFlow {
        id: usize,
        left: String,
        right: String,
        reject_negative: bool,
        fallback: i64,
    },
    IteratorControl {
        id: usize,
        values: Vec<i64>,
        parity: usize,
        limit: usize,
        skip_negative: bool,
    },
}

impl SemanticCase {
    pub fn prelude(&self) -> String {
        match self {
            Self::BorrowedVector { id, .. } => borrowed_vector_prelude(*id),
            Self::OwnedRecord { id, .. } => owned_record_prelude(*id),
            Self::ResultFlow { id, .. } => result_flow_prelude(*id),
            Self::IteratorControl { .. } => String::new(),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::BorrowedVector {
                id,
                values,
                delta,
                start,
                take,
            } => render_borrowed_vector(*id, values, *delta, *start, *take),
            Self::OwnedRecord {
                id,
                label,
                values,
                extra,
            } => render_owned_record(*id, label, values, *extra),
            Self::ResultFlow {
                id,
                left,
                right,
                reject_negative,
                fallback,
            } => render_result_flow(*id, left, right, *reject_negative, *fallback),
            Self::IteratorControl {
                id,
                values,
                parity,
                limit,
                skip_negative,
            } => render_iterator_control(*id, values, *parity, *limit, *skip_negative),
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let minimal = match self {
            Self::BorrowedVector { id, .. } => Self::BorrowedVector {
                id: *id,
                values: vec![0],
                delta: 0,
                start: 0,
                take: 1,
            },
            Self::OwnedRecord { id, .. } => Self::OwnedRecord {
                id: *id,
                label: String::new(),
                values: vec![0],
                extra: 0,
            },
            Self::ResultFlow { id, .. } => Self::ResultFlow {
                id: *id,
                left: "zero".to_string(),
                right: "zero".to_string(),
                reject_negative: false,
                fallback: 0,
            },
            Self::IteratorControl { id, .. } => Self::IteratorControl {
                id: *id,
                values: vec![0],
                parity: 0,
                limit: 1,
                skip_negative: false,
            },
        };
        if self == &minimal {
            Vec::new()
        } else {
            vec![minimal]
        }
    }

    pub fn shape(&self, output: &mut String) {
        output.push_str(match self {
            Self::BorrowedVector { .. } => "borrowed-vector",
            Self::OwnedRecord { .. } => "owned-record",
            Self::ResultFlow { .. } => "result-flow",
            Self::IteratorControl { .. } => "iterator-control",
        });
    }

    pub fn features(&self, output: &mut BTreeSet<&'static str>) {
        match self {
            Self::BorrowedVector { .. } => {
                output.extend(["borrow-mut", "borrow-shared", "slice", "iter-mut"]);
            }
            Self::OwnedRecord { .. } => {
                output.extend(["struct", "associated-function", "method", "move"]);
            }
            Self::ResultFlow { .. } => {
                output.extend(["result", "question-mark", "early-return"]);
            }
            Self::IteratorControl { .. } => {
                output.extend([
                    "iterator-enumerate",
                    "iterator-filter-map",
                    "iterator-take",
                    "loop",
                    "break",
                    "continue",
                ]);
            }
        }
    }
}

fn borrowed_vector_prelude(id: usize) -> String {
    format!(
        r#"fn generated_adjust_values_{id}(values: &mut Vec<i64>, delta: i64) {{
    for value in values.iter_mut() {{
        *value = (*value).saturating_add(delta);
    }}
}}

fn generated_sum_slice_{id}(values: &[i64]) -> i64 {{
    values
        .iter()
        .copied()
        .fold(0i64, |total, value| total.saturating_add(value))
}}

"#
    )
}

fn owned_record_prelude(id: usize) -> String {
    format!(
        r#"struct GeneratedRecord{id} {{
    label: String,
    values: Vec<i64>,
}}

impl GeneratedRecord{id} {{
    fn new(label: String, values: Vec<i64>) -> Self {{
        Self {{ label, values }}
    }}

    fn with_value(mut self, value: i64) -> Self {{
        self.values.push(value);
        self
    }}

    fn score(&self) -> i64 {{
        self.values
            .iter()
            .copied()
            .fold(0i64, |total, value| total.saturating_add(value))
    }}

    fn into_parts(self) -> (String, Vec<i64>) {{
        (self.label, self.values)
    }}
}}

"#
    )
}

fn result_flow_prelude(id: usize) -> String {
    format!(
        r#"fn generated_named_number_{id}(input: &str) -> Result<i64, String> {{
    match input.trim() {{
        "negative" => Ok(-1i64),
        "zero" => Ok(0i64),
        "one" => Ok(1i64),
        "large" => Ok(100i64),
        other => Err(format!("unknown:{{other}}")),
    }}
}}

fn generated_combine_result_{id}(
    left: &str,
    right: &str,
    reject_negative: bool,
) -> Result<i64, String> {{
    let left_value = generated_named_number_{id}(left)?;
    let right_value = generated_named_number_{id}(right)?;
    if reject_negative && (left_value < 0i64 || right_value < 0i64) {{
        return Err("negative".to_string());
    }}
    Ok(left_value.saturating_add(right_value))
}}

"#
    )
}

fn render_borrowed_vector(
    id: usize,
    values: &[i64],
    delta: i64,
    start: usize,
    take: usize,
) -> String {
    let values = i64_values(values);
    format!(
        r#"    {{
        let mut borrowed_values_{id}: Vec<i64> = vec![{values}];
        generated_adjust_values_{id}(&mut borrowed_values_{id}, {delta}i64);
        let borrowed_start_{id} = {start}usize.min(borrowed_values_{id}.len());
        let borrowed_end_{id} = borrowed_start_{id}
            .saturating_add({take}usize)
            .min(borrowed_values_{id}.len());
        let borrowed_slice_{id}: &[i64] =
            &borrowed_values_{id}[borrowed_start_{id}..borrowed_end_{id}];
        let borrowed_total_{id} = generated_sum_slice_{id}(borrowed_slice_{id});
        println!("semantic-borrowed-{id}:{{borrowed_values_{id}:?}}:{{borrowed_total_{id}}}");
    }}
"#
    )
}

fn render_owned_record(id: usize, label: &str, values: &[i64], extra: i64) -> String {
    let values = i64_values(values);
    format!(
        r#"    {{
        let owned_record_{id} = GeneratedRecord{id}::new(
            {label:?}.to_string(),
            vec![{values}],
        )
        .with_value({extra}i64);
        let owned_score_{id} = owned_record_{id}.score();
        let (owned_label_{id}, owned_values_{id}) = owned_record_{id}.into_parts();
        println!(
            "semantic-owned-{id}:{{owned_label_{id}}}:{{owned_score_{id}}}:{{owned_values_{id}:?}}"
        );
    }}
"#
    )
}

fn render_result_flow(
    id: usize,
    left: &str,
    right: &str,
    reject_negative: bool,
    fallback: i64,
) -> String {
    format!(
        r#"    {{
        let result_value_{id} =
            generated_combine_result_{id}({left:?}, {right:?}, {reject_negative});
        let result_display_{id} = match &result_value_{id} {{
            Ok(value) => format!("ok:{{value}}"),
            Err(error) => format!("err:{{error}}"),
        }};
        let result_number_{id} = result_value_{id}.unwrap_or({fallback}i64);
        println!("semantic-result-{id}:{{result_display_{id}}}:{{result_number_{id}}}");
    }}
"#
    )
}

fn render_iterator_control(
    id: usize,
    values: &[i64],
    parity: usize,
    limit: usize,
    skip_negative: bool,
) -> String {
    let values = i64_values(values);
    format!(
        r#"    {{
        let iterator_values_{id}: Vec<i64> = vec![{values}];
        let iterator_selected_{id}: Vec<i64> = iterator_values_{id}
            .into_iter()
            .enumerate()
            .filter_map(|(index, value)| {{
                if index % 2usize == {parity}usize {{
                    Some(value.saturating_add(index as i64))
                }} else {{
                    None
                }}
            }})
            .take({limit}usize)
            .collect();
        let mut iterator_index_{id} = 0usize;
        let mut iterator_total_{id} = 0i64;
        loop {{
            if iterator_index_{id} >= iterator_selected_{id}.len() {{
                break;
            }}
            let iterator_value_{id} = iterator_selected_{id}[iterator_index_{id}];
            iterator_index_{id} = iterator_index_{id}.saturating_add(1usize);
            if {skip_negative} && iterator_value_{id} < 0i64 {{
                continue;
            }}
            iterator_total_{id} = iterator_total_{id}.saturating_add(iterator_value_{id});
        }}
        println!(
            "semantic-iterator-{id}:{{iterator_selected_{id}:?}}:{{iterator_total_{id}}}"
        );
    }}
"#
    )
}

fn i64_values(values: &[i64]) -> String {
    values
        .iter()
        .map(|value| format!("{value}i64"))
        .collect::<Vec<_>>()
        .join(", ")
}
