//! Random data for the serde bridge. A struct shape and json documents with missing, extra and
//! wrong typed fields, read with `serde_json::from_str`. What the read gives back is printed, so
//! every error message and position is compared.

use std::collections::BTreeSet;

use rand::RngExt;
use rand::rngs::StdRng;
use serde::{Deserialize, Serialize};

use crate::lang::width::{INT_WIDTHS, IntWidth};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SerdeTy {
    Int(IntWidth),
    F64,
    Bool,
    Str,
    Opt(Box<SerdeTy>),
    Vec(Box<SerdeTy>),
}

impl SerdeTy {
    fn rust(&self) -> String {
        match self {
            Self::Int(width) => width.rust().to_string(),
            Self::F64 => "f64".to_string(),
            Self::Bool => "bool".to_string(),
            Self::Str => "String".to_string(),
            Self::Opt(inner) => format!("Option<{}>", inner.rust()),
            Self::Vec(inner) => format!("Vec<{}>", inner.rust()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SerdeField {
    pub name: String,
    pub ty: SerdeTy,
    /// `#[serde(default)]`, a missing field is then no error
    pub default: bool,
}

/// What is wrong with a document, for the feature report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Flaw {
    Missing,
    Extra,
    WrongType,
    Duplicate,
    Malformed,
}

impl Flaw {
    fn feature(self) -> &'static str {
        match self {
            Self::Missing => "lang-serde-missing-field",
            Self::Extra => "lang-serde-extra-field",
            Self::WrongType => "lang-serde-wrong-type",
            Self::Duplicate => "lang-serde-duplicate-field",
            Self::Malformed => "lang-serde-malformed",
        }
    }
}

/// How the read is written and what of it is printed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReadForm {
    /// `{:?}` of `serde_json::from_str::<T>(text)`
    Fish,
    /// `let x: Result<T, serde_json::Error> = serde_json::from_str(text);`, then `{:?}`
    Annotated,
    /// the value written back with `serde_json::to_string`, or the `Display` of the error
    RoundTrip,
}

impl ReadForm {
    fn feature(self) -> &'static str {
        match self {
            Self::Fish => "lang-serde-read-fish",
            Self::Annotated => "lang-serde-read-annotated",
            Self::RoundTrip => "lang-serde-round-trip",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SerdeInput {
    pub text: String,
    pub form: ReadForm,
    pub flaws: Vec<Flaw>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SerdeProbe {
    /// the struct name, it carries the block tag
    pub name: String,
    pub fields: Vec<SerdeField>,
    pub inputs: Vec<SerdeInput>,
}

impl SerdeProbe {
    pub fn generate(rng: &mut StdRng, name: String) -> Self {
        let count = rng.random_range(1..=4);
        let fields: Vec<SerdeField> = (0..count)
            .map(|slot| SerdeField {
                name: format!("f{slot}"),
                ty: field_ty(rng, 2),
                default: rng.random_bool(0.2),
            })
            .collect();
        let inputs = (0..rng.random_range(2..=4))
            .map(|_| input(rng, &fields))
            .collect();
        Self {
            name,
            fields,
            inputs,
        }
    }

    /// The struct, above `main`.
    pub fn render_items(&self) -> String {
        let mut out = format!(
            "#[derive(Debug, serde::Deserialize, serde::Serialize)]\nstruct {} {{\n",
            self.name
        );
        for field in &self.fields {
            if field.default {
                out.push_str("    #[serde(default)]\n");
            }
            out.push_str(&format!("    {}: {},\n", field.name, field.ty.rust()));
        }
        out.push_str("}\n\n");
        out
    }

    /// One read and one print per document, inside `main`.
    pub fn render(&self) -> String {
        let name = &self.name;
        let mut out = String::new();
        for (index, input) in self.inputs.iter().enumerate() {
            let label = format!("{name}_{index}");
            let text = format!("r#\"{}\"#", input.text);
            out.push_str(&match input.form {
                ReadForm::Fish => format!(
                    "    println!(\"{label}: {{:?}}\", serde_json::from_str::<{name}>({text}));\n"
                ),
                ReadForm::Annotated => format!(
                    "    {{\n        let diff_read: Result<{name}, serde_json::Error> = serde_json::from_str({text});\n        println!(\"{label}: {{:?}}\", diff_read);\n    }}\n"
                ),
                ReadForm::RoundTrip => format!(
                    "    match serde_json::from_str::<{name}>({text}) {{\n        Ok(diff_read) => println!(\"{label}: {{:?}}\", serde_json::to_string(&diff_read)),\n        Err(diff_error) => println!(\"{label}: {{}}\", diff_error),\n    }}\n"
                ),
            });
        }
        out
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert("lang-serde-json");
        if self.fields.iter().any(|field| field.default) {
            out.insert("lang-serde-default-attr");
        }
        for input in &self.inputs {
            out.insert(input.form.feature());
            out.extend(input.flaws.iter().map(|flaw| flaw.feature()));
        }
    }

    /// Smaller probes for the reducer, one document less each.
    pub fn shrinks(&self) -> Vec<Self> {
        if self.inputs.len() < 2 {
            return Vec::new();
        }
        (0..self.inputs.len())
            .map(|index| {
                let mut smaller = self.clone();
                smaller.inputs.remove(index);
                smaller
            })
            .collect()
    }
}

fn field_ty(rng: &mut StdRng, depth: usize) -> SerdeTy {
    let pick = if depth == 0 {
        rng.random_range(0..7)
    } else {
        rng.random_range(0..10)
    };
    match pick {
        0..=2 => SerdeTy::Int(INT_WIDTHS[rng.random_range(0..INT_WIDTHS.len())]),
        3 => SerdeTy::F64,
        4 => SerdeTy::Bool,
        5 | 6 => SerdeTy::Str,
        7 | 8 => SerdeTy::Opt(Box::new(field_ty(rng, depth - 1))),
        _ => SerdeTy::Vec(Box::new(field_ty(rng, depth - 1))),
    }
}

fn pick<'a>(rng: &mut StdRng, items: &[&'a str]) -> &'a str {
    items[rng.random_range(0..items.len())]
}

const STRINGS: [&str; 6] = [
    r#""""#,
    r#""a""#,
    r#""two words""#,
    r#""hé""#,
    r#""line\nbreak""#,
    r#""q\"uote""#,
];

/// A json value the type reads.
fn good(rng: &mut StdRng, ty: &SerdeTy) -> String {
    match ty {
        SerdeTy::Int(width) => match rng.random_range(0..5) {
            0 => width.min().to_string(),
            1 => width.max().to_string(),
            2 => "0".to_string(),
            3 => "1".to_string(),
            _ => rng.random_range(0..=100u8).to_string(),
        },
        SerdeTy::F64 => pick(rng, &["0.5", "-1.25", "1e3", "3", "2.0", "-0.0"]).to_string(),
        SerdeTy::Bool => pick(rng, &["true", "false"]).to_string(),
        SerdeTy::Str => pick(rng, &STRINGS).to_string(),
        SerdeTy::Opt(inner) => {
            if rng.random_bool(0.3) {
                "null".to_string()
            } else {
                good(rng, inner)
            }
        }
        SerdeTy::Vec(inner) => {
            let items: Vec<String> = (0..rng.random_range(0..=3))
                .map(|_| good(rng, inner))
                .collect();
            format!("[{}]", items.join(", "))
        }
    }
}

/// A json value the type refuses.
fn wrong(rng: &mut StdRng, ty: &SerdeTy) -> String {
    match ty {
        SerdeTy::Int(width) => match rng.random_range(0..6) {
            0 => (width.max() + 1).to_string(),
            1 => (width.min() - 1).to_string(),
            2 => "1.5".to_string(),
            3 => r#""7""#.to_string(),
            4 => "true".to_string(),
            _ => "[1]".to_string(),
        },
        SerdeTy::F64 => pick(rng, &[r#""1.0""#, "true", "[]", "{}"]).to_string(),
        SerdeTy::Bool => pick(rng, &[r#""true""#, "0", "1", "null"]).to_string(),
        SerdeTy::Str => pick(rng, &["1", "null", "true", r#"["a"]"#, "2.5"]).to_string(),
        SerdeTy::Opt(inner) => wrong(rng, inner),
        SerdeTy::Vec(inner) => match rng.random_range(0..4) {
            0 => "{}".to_string(),
            1 => "3".to_string(),
            2 => r#""abc""#.to_string(),
            // the flaw sits at the second item, so the position in the message moves
            _ => format!("[{}, {}]", good(rng, inner), wrong(rng, inner)),
        },
    }
}

fn input(rng: &mut StdRng, fields: &[SerdeField]) -> SerdeInput {
    let mut flaws = Vec::new();
    let mut members: Vec<String> = Vec::new();
    for field in fields {
        let value = match rng.random_range(0..10) {
            0 => {
                flaws.push(Flaw::Missing);
                continue;
            }
            1 => {
                flaws.push(Flaw::WrongType);
                wrong(rng, &field.ty)
            }
            _ => good(rng, &field.ty),
        };
        members.push(format!("\"{}\": {value}", field.name));
    }
    if rng.random_bool(0.15) {
        flaws.push(Flaw::Extra);
        let value = pick(
            rng,
            &["1", r#""x""#, "null", "[1, [2]]", r#"{"a": {"b": 1}}"#],
        );
        let at = rng.random_range(0..=members.len());
        members.insert(at, format!("\"extra\": {value}"));
    }
    if rng.random_bool(0.08)
        && let Some(first) = members.first().cloned()
    {
        flaws.push(Flaw::Duplicate);
        members.push(first);
    }
    if rng.random_bool(0.2) {
        members.reverse();
    }
    // a line break moves the line and the column of every later error
    let separator = pick(rng, &[", ", ",", ",\n  ", " ,\n"]);
    let mut text = format!("{{{}}}", members.join(separator));
    if rng.random_bool(0.1) {
        flaws.push(Flaw::Malformed);
        text = match rng.random_range(0..6) {
            0 => text.trim_end_matches('}').to_string(),
            1 => text.replacen('{', "{,", 1),
            2 => format!("{text} x"),
            3 => String::new(),
            4 => "null".to_string(),
            _ => format!("[{text}]"),
        };
    }
    flaws.sort();
    flaws.dedup();
    let form = match rng.random_range(0..3) {
        0 => ReadForm::Fish,
        1 => ReadForm::Annotated,
        _ => ReadForm::RoundTrip,
    };
    SerdeInput { text, form, flaws }
}
