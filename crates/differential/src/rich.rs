use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RichCase {
    OptionClosure {
        input: Option<i64>,
        threshold: i64,
        multiplier: i64,
        offset: i64,
        fallback: Option<i64>,
    },
    VectorPipeline {
        values: Vec<i64>,
        multiplier: i64,
        offset: i64,
        minimum: i64,
        extra: i64,
        lookup: usize,
        reverse: bool,
    },
    PathString {
        raw: String,
        child: Option<String>,
        extension: String,
        separator: char,
        needle: String,
        replacement: String,
        uppercase: bool,
    },
    EnumMatch {
        variant: StateVariant,
        label: String,
        path: String,
        values: Vec<i64>,
        needle: String,
        bias: i64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StateVariant {
    Idle,
    Named,
    Located,
}

impl RichCase {
    pub fn render(&self) -> String {
        match self {
            Self::OptionClosure {
                input,
                threshold,
                multiplier,
                offset,
                fallback,
            } => render_option(*input, *threshold, *multiplier, *offset, *fallback),
            Self::VectorPipeline {
                values,
                multiplier,
                offset,
                minimum,
                extra,
                lookup,
                reverse,
            } => render_vector(
                values,
                *multiplier,
                *offset,
                *minimum,
                *extra,
                *lookup,
                *reverse,
            ),
            Self::PathString {
                raw,
                child,
                extension,
                separator,
                needle,
                replacement,
                uppercase,
            } => render_path(
                raw,
                child.as_deref(),
                extension,
                *separator,
                needle,
                replacement,
                *uppercase,
            ),
            Self::EnumMatch {
                variant,
                label,
                path,
                values,
                needle,
                bias,
            } => render_enum(*variant, label, path, values, needle, *bias),
        }
    }

    pub fn requires_path(&self) -> bool {
        matches!(self, Self::PathString { .. } | Self::EnumMatch { .. })
    }

    pub fn requires_generated_state(&self) -> bool {
        matches!(self, Self::EnumMatch { .. })
    }

    pub fn shrinks(&self) -> Vec<Self> {
        match self {
            Self::OptionClosure {
                input,
                threshold,
                multiplier,
                offset,
                fallback,
            } => {
                let mut candidates = Vec::new();
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::OptionClosure {
                        input: None,
                        threshold: *threshold,
                        multiplier: *multiplier,
                        offset: *offset,
                        fallback: *fallback,
                    },
                );
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::OptionClosure {
                        input: *input,
                        threshold: 0,
                        multiplier: 1,
                        offset: 0,
                        fallback: None,
                    },
                );
                candidates
            }
            Self::VectorPipeline {
                values,
                multiplier,
                offset,
                minimum,
                extra,
                lookup,
                reverse,
            } => {
                let mut candidates = Vec::new();
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::VectorPipeline {
                        values: Vec::new(),
                        multiplier: *multiplier,
                        offset: *offset,
                        minimum: *minimum,
                        extra: *extra,
                        lookup: *lookup,
                        reverse: *reverse,
                    },
                );
                if values.len() > 1 {
                    push_if_changed(
                        &mut candidates,
                        self,
                        Self::VectorPipeline {
                            values: vec![values[0]],
                            multiplier: *multiplier,
                            offset: *offset,
                            minimum: *minimum,
                            extra: *extra,
                            lookup: *lookup,
                            reverse: *reverse,
                        },
                    );
                }
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::VectorPipeline {
                        values: values.clone(),
                        multiplier: 1,
                        offset: 0,
                        minimum: 0,
                        extra: 0,
                        lookup: 0,
                        reverse: false,
                    },
                );
                candidates
            }
            Self::PathString {
                raw,
                child,
                extension,
                separator,
                needle,
                replacement,
                uppercase,
            } => {
                let mut candidates = Vec::new();
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::PathString {
                        raw: "a.txt".to_string(),
                        child: child.clone(),
                        extension: extension.clone(),
                        separator: *separator,
                        needle: needle.clone(),
                        replacement: replacement.clone(),
                        uppercase: *uppercase,
                    },
                );
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::PathString {
                        raw: raw.clone(),
                        child: None,
                        extension: String::new(),
                        separator: '_',
                        needle: String::new(),
                        replacement: String::new(),
                        uppercase: false,
                    },
                );
                candidates
            }
            Self::EnumMatch {
                variant,
                label,
                path,
                values,
                needle,
                bias,
            } => {
                let mut candidates = Vec::new();
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::EnumMatch {
                        variant: StateVariant::Idle,
                        label: label.clone(),
                        path: path.clone(),
                        values: values.clone(),
                        needle: needle.clone(),
                        bias: *bias,
                    },
                );
                push_if_changed(
                    &mut candidates,
                    self,
                    Self::EnumMatch {
                        variant: *variant,
                        label: String::new(),
                        path: "a".to_string(),
                        values: Vec::new(),
                        needle: String::new(),
                        bias: 0,
                    },
                );
                candidates
            }
        }
    }
}

fn render_option(
    input: Option<i64>,
    threshold: i64,
    multiplier: i64,
    offset: i64,
    fallback: Option<i64>,
) -> String {
    let input = option_i64(input);
    let fallback = option_i64(fallback);
    format!(
        r#"    {{
        let option_input: Option<i64> = {input};
        let option_multiplier = {multiplier}i64;
        let option_transform = |value: i64| {{
            value
                .saturating_mul(option_multiplier)
                .saturating_add({offset}i64)
        }};
        let option_output = option_input
            .filter(|value| *value >= {threshold}i64)
            .map(option_transform)
            .or_else(|| {fallback});
        let option_label = match option_output {{
            Some(value) if value % 2i64 == 0i64 => format!("even:{{value}}"),
            Some(value) => format!("odd:{{value}}"),
            None => "none".to_string(),
        }};
        println!("option:{{option_output:?}}:{{option_label}}");
    }}
"#
    )
}

fn render_vector(
    values: &[i64],
    multiplier: i64,
    offset: i64,
    minimum: i64,
    extra: i64,
    lookup: usize,
    reverse: bool,
) -> String {
    let values = i64_values(values);
    let order = if reverse {
        "vector_output.reverse();"
    } else {
        "vector_output.sort();"
    };
    format!(
        r#"    {{
        let vector_input: Vec<i64> = vec![{values}];
        let vector_multiplier = {multiplier}i64;
        let vector_offset = {offset}i64;
        let vector_transform = |value: i64| {{
            value
                .saturating_mul(vector_multiplier)
                .saturating_add(vector_offset)
        }};
        let mut vector_output: Vec<i64> = vector_input
            .iter()
            .copied()
            .map(vector_transform)
            .filter(|value| *value >= {minimum}i64)
            .collect();
        vector_output.push({extra}i64);
        {order}
        let vector_selected = vector_output
            .get({lookup}usize)
            .copied()
            .unwrap_or(-1i64);
        let vector_shape = match vector_output.len() {{
            0usize => "empty".to_string(),
            1usize => format!("one:{{vector_selected}}"),
            length => format!("many:{{length}}:{{vector_selected}}"),
        }};
        println!("vector:{{vector_output:?}}:{{vector_shape}}");
    }}
"#
    )
}

fn render_path(
    raw: &str,
    child: Option<&str>,
    extension: &str,
    separator: char,
    needle: &str,
    replacement: &str,
    uppercase: bool,
) -> String {
    let raw = string_literal(raw);
    let child = option_string(child);
    let extension = string_literal(extension);
    let separator = format!("{separator:?}");
    let needle = string_literal(needle);
    let replacement = string_literal(replacement);
    let transform = if uppercase {
        "piece.to_uppercase()"
    } else {
        "piece.to_ascii_lowercase()"
    };
    format!(
        r#"    {{
        let path_raw = {raw};
        let path_normalized = path_raw
            .trim()
            .replace('\\', "/");
        let path_base = PathBuf::from(path_normalized);
        let path_child: Option<String> = {child};
        let path_joined = match path_child {{
            Some(child) => path_base.join(child),
            None => path_base,
        }};
        let path_changed = path_joined.with_extension({extension});
        let path_parent = path_changed
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default();
        let path_file = path_changed
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let path_parts: Vec<String> = path_file
            .split({separator})
            .filter(|piece| !piece.is_empty())
            .map(|piece| {transform}.replace({needle}, {replacement}))
            .collect();
        println!(
            "path:{{}}:{{path_parent}}:{{path_parts:?}}",
            path_changed.display()
        );
    }}
"#
    )
}

fn render_enum(
    variant: StateVariant,
    label: &str,
    path: &str,
    values: &[i64],
    needle: &str,
    bias: i64,
) -> String {
    let state = match variant {
        StateVariant::Idle => "GeneratedState::Idle".to_string(),
        StateVariant::Named => {
            format!(
                "GeneratedState::Named({}.to_string())",
                string_literal(label)
            )
        }
        StateVariant::Located => format!(
            "GeneratedState::Located {{ path: PathBuf::from({}), values: vec![{}] }}",
            string_literal(path),
            i64_values(values)
        ),
    };
    let needle = string_literal(needle);
    format!(
        r#"    {{
        let enum_state = {state};
        let enum_bias = {bias}i64;
        let enum_sum = |values: Vec<i64>| {{
            values
                .iter()
                .copied()
                .fold(enum_bias, |total, value| total.saturating_add(value))
        }};
        let enum_label = match enum_state {{
            GeneratedState::Idle => "idle".to_string(),
            GeneratedState::Named(name) if name.contains({needle}) => {{
                format!("matched:{{}}", name.to_uppercase())
            }}
            GeneratedState::Named(name) => format!("named:{{name}}"),
            GeneratedState::Located {{ path, values }} => {{
                let total = enum_sum(values);
                format!("located:{{}}:{{total}}", path.display())
            }}
        }};
        println!("enum:{{enum_label}}");
    }}
"#
    )
}

fn push_if_changed(candidates: &mut Vec<RichCase>, current: &RichCase, candidate: RichCase) {
    if current != &candidate {
        candidates.push(candidate);
    }
}

fn option_i64(value: Option<i64>) -> String {
    match value {
        Some(value) => format!("Some({value}i64)"),
        None => "None".to_string(),
    }
}

fn option_string(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("Some({}.to_string())", string_literal(value)),
        None => "None".to_string(),
    }
}

fn string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn i64_values(values: &[i64]) -> String {
    values
        .iter()
        .map(|value| format!("{value}i64"))
        .collect::<Vec<_>>()
        .join(", ")
}
