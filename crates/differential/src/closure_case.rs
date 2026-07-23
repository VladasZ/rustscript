use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClosureCase {
    Nested {
        input: i64,
        outer_bias: i64,
        inner_bias: i64,
        arguments: [i64; 2],
    },
    MutableCapture {
        values: Vec<i64>,
        initial: i64,
    },
    MoveString {
        prefix: String,
        suffixes: Vec<String>,
    },
    CapturedCall {
        values: Vec<i64>,
        bias: i64,
        threshold: i64,
    },
    TuplePattern {
        pairs: Vec<(i64, i64)>,
        multiplier: i64,
    },
    GenericApply {
        input: i64,
        delta: i64,
        times: usize,
    },
}

impl ClosureCase {
    pub fn render(&self) -> String {
        match self {
            Self::Nested {
                input,
                outer_bias,
                inner_bias,
                arguments,
            } => render_nested(*input, *outer_bias, *inner_bias, *arguments),
            Self::MutableCapture { values, initial } => render_mutable_capture(values, *initial),
            Self::MoveString { prefix, suffixes } => render_move_string(prefix, suffixes),
            Self::CapturedCall {
                values,
                bias,
                threshold,
            } => render_captured_call(values, *bias, *threshold),
            Self::TuplePattern { pairs, multiplier } => render_tuple_pattern(pairs, *multiplier),
            Self::GenericApply {
                input,
                delta,
                times,
            } => render_generic_apply(*input, *delta, *times),
        }
    }

    pub fn requires_apply_helper(&self) -> bool {
        matches!(self, Self::GenericApply { .. })
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let minimal = match self {
            Self::Nested { .. } => Self::Nested {
                input: 0,
                outer_bias: 0,
                inner_bias: 0,
                arguments: [0, 1],
            },
            Self::MutableCapture { .. } => Self::MutableCapture {
                values: vec![0],
                initial: 0,
            },
            Self::MoveString { .. } => Self::MoveString {
                prefix: String::new(),
                suffixes: vec![String::new()],
            },
            Self::CapturedCall { .. } => Self::CapturedCall {
                values: vec![0],
                bias: 0,
                threshold: 0,
            },
            Self::TuplePattern { .. } => Self::TuplePattern {
                pairs: vec![(0, 0)],
                multiplier: 1,
            },
            Self::GenericApply { .. } => Self::GenericApply {
                input: 0,
                delta: 0,
                times: 1,
            },
        };
        if self == &minimal {
            Vec::new()
        } else {
            vec![minimal]
        }
    }
}

pub fn apply_helper() -> &'static str {
    r#"fn apply_generated<F>(mut operation: F, value: i64, times: usize) -> i64
where
    F: FnMut(i64) -> i64,
{
    let mut current = value;
    let mut remaining = times;
    while remaining > 0usize {
        current = operation(current);
        remaining = remaining.saturating_sub(1usize);
    }
    current
}

"#
}

fn render_nested(input: i64, outer_bias: i64, inner_bias: i64, arguments: [i64; 2]) -> String {
    let [first, second] = arguments;
    format!(
        r#"    {{
        let nested_outer_bias = {outer_bias}i64;
        let nested_make = |left: i64| {{
            let captured = left.saturating_add(nested_outer_bias);
            move |right: i64| {{
                captured
                    .saturating_mul(right)
                    .saturating_add({inner_bias}i64)
            }}
        }};
        let nested_operation = nested_make({input}i64);
        let nested_first = nested_operation({first}i64);
        let nested_second = nested_operation({second}i64);
        println!("closure-nested:{{nested_first}}:{{nested_second}}");
    }}
"#
    )
}

fn render_mutable_capture(values: &[i64], initial: i64) -> String {
    let values = i64_values(values);
    format!(
        r#"    {{
        let mutable_input: Vec<i64> = vec![{values}];
        let mut mutable_total = {initial}i64;
        let mutable_output: Vec<i64> = {{
            let mut accumulate = |value: i64| {{
                mutable_total = mutable_total.saturating_add(value);
                mutable_total
            }};
            mutable_input
                .iter()
                .copied()
                .map(|value| accumulate(value))
                .collect()
        }};
        println!("closure-mutable:{{mutable_total}}:{{mutable_output:?}}");
    }}
"#
    )
}

fn render_move_string(prefix: &str, suffixes: &[String]) -> String {
    let prefix = string_literal(prefix);
    let suffixes = string_values(suffixes);
    format!(
        r#"    {{
        let move_prefix = {prefix}.to_string();
        let move_suffixes: Vec<String> = vec![{suffixes}];
        let decorate = move |(index, suffix): (usize, String)| {{
            format!(
                "{{}}:{{index}}:{{}}",
                move_prefix.to_uppercase(),
                suffix.trim()
            )
        }};
        let move_output: Vec<String> = move_suffixes
            .into_iter()
            .enumerate()
            .map(decorate)
            .collect();
        println!("closure-move:{{move_output:?}}");
    }}
"#
    )
}

fn render_captured_call(values: &[i64], bias: i64, threshold: i64) -> String {
    let values = i64_values(values);
    format!(
        r#"    {{
        let captured_input: Vec<i64> = vec![{values}];
        let captured_base = |value: i64| value.saturating_add({bias}i64);
        let captured_select = |value: i64| {{
            let adjusted = captured_base(value);
            match adjusted {{
                selected if selected >= {threshold}i64 => Some(selected),
                _ => None,
            }}
        }};
        let captured_output: Vec<i64> = captured_input
            .into_iter()
            .filter_map(captured_select)
            .collect();
        println!("closure-captured:{{captured_output:?}}");
    }}
"#
    )
}

fn render_tuple_pattern(pairs: &[(i64, i64)], multiplier: i64) -> String {
    let pairs = pairs
        .iter()
        .map(|(left, right)| format!("({left}i64, {right}i64)"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"    {{
        let tuple_input: Vec<(i64, i64)> = vec![{pairs}];
        let tuple_combine = |(left, right): (i64, i64)| {{
            left.saturating_add(right)
                .saturating_mul({multiplier}i64)
        }};
        let tuple_output: Vec<i64> = tuple_input
            .into_iter()
            .map(tuple_combine)
            .collect();
        println!("closure-tuple:{{tuple_output:?}}");
    }}
"#
    )
}

fn render_generic_apply(input: i64, delta: i64, times: usize) -> String {
    format!(
        r#"    {{
        let mut generic_calls = 0i64;
        let generic_operation = |value: i64| {{
            generic_calls = generic_calls.saturating_add(1i64);
            value.saturating_add({delta}i64)
        }};
        let generic_output =
            apply_generated(generic_operation, {input}i64, {times}usize);
        println!("closure-generic:{{generic_calls}}:{{generic_output}}");
    }}
"#
    )
}

fn string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn string_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("{}.to_string()", string_literal(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn i64_values(values: &[i64]) -> String {
    values
        .iter()
        .map(|value| format!("{value}i64"))
        .collect::<Vec<_>>()
        .join(", ")
}
