use std::collections::BTreeSet;

use rand::RngExt;
use rand::rngs::StdRng;
use serde::{Deserialize, Serialize};

/// One bridged-method call from the catalog, printed as its own labeled line.
/// The receiver is `text`, `values`, or the map, depending on the variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MethodCall {
    StrLen,
    StrIsEmpty,
    StrTrim,
    StrTrimStart,
    StrTrimEnd,
    StrUpper,
    StrLower,
    StrAsciiUpper,
    StrAsciiLower,
    StrRepeat,
    StrContains,
    StrStartsWith,
    StrEndsWith,
    StrFind,
    StrRfind,
    StrReplace,
    StrReplacen,
    StrSplitCount,
    StrSplitn,
    StrSplitOnce,
    StrRsplitOnce,
    StrStripPrefix,
    StrStripSuffix,
    StrTrimStartMatches,
    StrTrimEndMatches,
    StrEqIgnoreCase,
    StrMatchesCount,
    StrCharsCount,
    StrCharsRev,
    StrCharIndices,
    StrLines,
    StrSplitWhitespace,
    VecLen,
    VecIsEmpty,
    VecContains,
    VecFirst,
    VecLast,
    VecMax,
    VecMin,
    VecSum,
    VecProduct,
    VecRev,
    VecSorted,
    VecDedupSorted,
    VecPosition,
    VecAny,
    VecAll,
    MapLen,
    MapContainsKey,
    MapGet,
    MapRemoveLen,
    MapSortedKeys,
    MapValuesSum,
}

const CATALOG: &[MethodCall] = &[
    MethodCall::StrLen,
    MethodCall::StrIsEmpty,
    MethodCall::StrTrim,
    MethodCall::StrTrimStart,
    MethodCall::StrTrimEnd,
    MethodCall::StrUpper,
    MethodCall::StrLower,
    MethodCall::StrAsciiUpper,
    MethodCall::StrAsciiLower,
    MethodCall::StrRepeat,
    MethodCall::StrContains,
    MethodCall::StrStartsWith,
    MethodCall::StrEndsWith,
    MethodCall::StrFind,
    MethodCall::StrRfind,
    MethodCall::StrReplace,
    MethodCall::StrReplacen,
    MethodCall::StrSplitCount,
    MethodCall::StrSplitn,
    MethodCall::StrSplitOnce,
    MethodCall::StrRsplitOnce,
    MethodCall::StrStripPrefix,
    MethodCall::StrStripSuffix,
    MethodCall::StrTrimStartMatches,
    MethodCall::StrTrimEndMatches,
    MethodCall::StrEqIgnoreCase,
    MethodCall::StrMatchesCount,
    MethodCall::StrCharsCount,
    MethodCall::StrCharsRev,
    MethodCall::StrCharIndices,
    MethodCall::StrLines,
    MethodCall::StrSplitWhitespace,
    MethodCall::VecLen,
    MethodCall::VecIsEmpty,
    MethodCall::VecContains,
    MethodCall::VecFirst,
    MethodCall::VecLast,
    MethodCall::VecMax,
    MethodCall::VecMin,
    MethodCall::VecSum,
    MethodCall::VecProduct,
    MethodCall::VecRev,
    MethodCall::VecSorted,
    MethodCall::VecDedupSorted,
    MethodCall::VecPosition,
    MethodCall::VecAny,
    MethodCall::VecAll,
    MethodCall::MapLen,
    MethodCall::MapContainsKey,
    MethodCall::MapGet,
    MethodCall::MapRemoveLen,
    MethodCall::MapSortedKeys,
    MethodCall::MapValuesSum,
];

impl MethodCall {
    fn name(self) -> &'static str {
        match self {
            Self::StrLen => "str-len",
            Self::StrIsEmpty => "str-is-empty",
            Self::StrTrim => "str-trim",
            Self::StrTrimStart => "str-trim-start",
            Self::StrTrimEnd => "str-trim-end",
            Self::StrUpper => "str-upper",
            Self::StrLower => "str-lower",
            Self::StrAsciiUpper => "str-ascii-upper",
            Self::StrAsciiLower => "str-ascii-lower",
            Self::StrRepeat => "str-repeat",
            Self::StrContains => "str-contains",
            Self::StrStartsWith => "str-starts-with",
            Self::StrEndsWith => "str-ends-with",
            Self::StrFind => "str-find",
            Self::StrRfind => "str-rfind",
            Self::StrReplace => "str-replace",
            Self::StrReplacen => "str-replacen",
            Self::StrSplitCount => "str-split-count",
            Self::StrSplitn => "str-splitn",
            Self::StrSplitOnce => "str-split-once",
            Self::StrRsplitOnce => "str-rsplit-once",
            Self::StrStripPrefix => "str-strip-prefix",
            Self::StrStripSuffix => "str-strip-suffix",
            Self::StrTrimStartMatches => "str-trim-start-matches",
            Self::StrTrimEndMatches => "str-trim-end-matches",
            Self::StrEqIgnoreCase => "str-eq-ignore-case",
            Self::StrMatchesCount => "str-matches-count",
            Self::StrCharsCount => "str-chars-count",
            Self::StrCharsRev => "str-chars-rev",
            Self::StrCharIndices => "str-char-indices",
            Self::StrLines => "str-lines",
            Self::StrSplitWhitespace => "str-split-whitespace",
            Self::VecLen => "vec-len-method",
            Self::VecIsEmpty => "vec-is-empty",
            Self::VecContains => "vec-contains",
            Self::VecFirst => "vec-first",
            Self::VecLast => "vec-last",
            Self::VecMax => "vec-max",
            Self::VecMin => "vec-min",
            Self::VecSum => "vec-sum",
            Self::VecProduct => "vec-product",
            Self::VecRev => "vec-rev",
            Self::VecSorted => "vec-sorted",
            Self::VecDedupSorted => "vec-dedup",
            Self::VecPosition => "vec-position",
            Self::VecAny => "vec-any",
            Self::VecAll => "vec-all",
            Self::MapLen => "map-len",
            Self::MapContainsKey => "map-contains-key",
            Self::MapGet => "map-get",
            Self::MapRemoveLen => "map-remove-len",
            Self::MapSortedKeys => "map-sorted-keys",
            Self::MapValuesSum => "map-values-sum",
        }
    }

    fn is_str(self) -> bool {
        self.name().starts_with("str-")
    }

    fn is_vec(self) -> bool {
        self.name().starts_with("vec-")
    }

    fn is_map(self) -> bool {
        self.name().starts_with("map-")
    }

    /// The expression printed for this call. `text`, `values`, and `map` are
    /// the receiver binding names, `needle` is rendered as a string literal
    /// and `probe` as an integer literal.
    fn expression(self, text: &str, values: &str, map: &str, needle: &str, probe: i64) -> String {
        match self {
            Self::StrLen => format!("{text}.len()"),
            Self::StrIsEmpty => format!("{text}.is_empty()"),
            Self::StrTrim => format!("{text}.trim()"),
            Self::StrTrimStart => format!("{text}.trim_start()"),
            Self::StrTrimEnd => format!("{text}.trim_end()"),
            Self::StrUpper => format!("{text}.to_uppercase()"),
            Self::StrLower => format!("{text}.to_lowercase()"),
            Self::StrAsciiUpper => format!("{text}.to_ascii_uppercase()"),
            Self::StrAsciiLower => format!("{text}.to_ascii_lowercase()"),
            Self::StrRepeat => format!("{text}.repeat(2usize)"),
            Self::StrContains => format!("{text}.contains({needle:?})"),
            Self::StrStartsWith => format!("{text}.starts_with({needle:?})"),
            Self::StrEndsWith => format!("{text}.ends_with({needle:?})"),
            Self::StrFind => format!("{text}.find({needle:?})"),
            Self::StrRfind => format!("{text}.rfind({needle:?})"),
            Self::StrReplace => format!("{text}.replace({needle:?}, \"x\")"),
            Self::StrReplacen => format!("{text}.replacen({needle:?}, \"y\", 1usize)"),
            Self::StrSplitCount => format!("{text}.split({needle:?}).count()"),
            Self::StrSplitn => {
                format!("{text}.splitn(2usize, {needle:?}).collect::<Vec<&str>>()")
            }
            Self::StrSplitOnce => format!("{text}.split_once({needle:?})"),
            Self::StrRsplitOnce => format!("{text}.rsplit_once({needle:?})"),
            Self::StrStripPrefix => format!("{text}.strip_prefix({needle:?})"),
            Self::StrStripSuffix => format!("{text}.strip_suffix({needle:?})"),
            Self::StrTrimStartMatches => format!("{text}.trim_start_matches({needle:?})"),
            Self::StrTrimEndMatches => format!("{text}.trim_end_matches({needle:?})"),
            Self::StrEqIgnoreCase => format!("{text}.eq_ignore_ascii_case({needle:?})"),
            Self::StrMatchesCount => format!("{text}.matches({needle:?}).count()"),
            Self::StrCharsCount => format!("{text}.chars().count()"),
            Self::StrCharsRev => format!("{text}.chars().rev().collect::<String>()"),
            Self::StrCharIndices => {
                format!("{text}.char_indices().take(3usize).collect::<Vec<(usize, char)>>()")
            }
            Self::StrLines => format!("{text}.lines().count()"),
            Self::StrSplitWhitespace => {
                format!("{text}.split_whitespace().collect::<Vec<&str>>()")
            }
            Self::VecLen => format!("{values}.len()"),
            Self::VecIsEmpty => format!("{values}.is_empty()"),
            Self::VecContains => format!("{values}.contains(&{probe}i64)"),
            Self::VecFirst => format!("{values}.first().copied()"),
            Self::VecLast => format!("{values}.last().copied()"),
            Self::VecMax => format!("{values}.iter().max().copied()"),
            Self::VecMin => format!("{values}.iter().min().copied()"),
            Self::VecSum => format!("{values}.iter().sum::<i64>()"),
            Self::VecProduct => format!("{values}.iter().product::<i64>()"),
            Self::VecRev => format!("{values}.iter().rev().copied().collect::<Vec<i64>>()"),
            Self::VecSorted => {
                format!("{{ let mut sorted = {values}.clone(); sorted.sort(); sorted }}")
            }
            Self::VecDedupSorted => format!(
                "{{ let mut sorted = {values}.clone(); sorted.sort(); sorted.dedup(); sorted }}"
            ),
            Self::VecPosition => {
                format!("{values}.iter().position(|&value| value == {probe}i64)")
            }
            Self::VecAny => format!("{values}.iter().any(|&value| value > {probe}i64)"),
            Self::VecAll => format!("{values}.iter().all(|&value| value < {probe}i64)"),
            Self::MapLen => format!("{map}.len()"),
            Self::MapContainsKey => format!("{map}.contains_key({needle:?})"),
            Self::MapGet => format!("{map}.get({needle:?}).copied()"),
            Self::MapRemoveLen => format!(
                "{{ let mut trimmed = {map}.clone(); trimmed.remove({needle:?}); trimmed.len() }}"
            ),
            Self::MapSortedKeys => format!(
                "{{ let mut keys = {map}.keys().collect::<Vec<&String>>(); keys.sort(); format!(\"{{keys:?}}\") }}"
            ),
            Self::MapValuesSum => format!("{map}.values().sum::<i64>()"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MethodsCase {
    pub id: usize,
    pub text: String,
    pub needle: String,
    pub values: Vec<i64>,
    pub probe: i64,
    pub entries: Vec<(String, i64)>,
    pub calls: Vec<MethodCall>,
}

impl MethodsCase {
    pub fn render(&self) -> String {
        let id = self.id;
        let text = format!("method_text_{id}");
        let values = format!("method_values_{id}");
        let map = format!("method_map_{id}");
        let mut source = String::from("    {\n");
        if self.calls.iter().any(|call| call.is_str()) {
            source.push_str(&format!(
                "        let {text}: String = {:?}.to_string();\n",
                self.text
            ));
        }
        if self.calls.iter().any(|call| call.is_vec()) {
            let items = self
                .values
                .iter()
                .map(|value| format!("{value}i64"))
                .collect::<Vec<_>>()
                .join(", ");
            source.push_str(&format!(
                "        let {values}: Vec<i64> = vec![{items}];\n"
            ));
        }
        if self.calls.iter().any(|call| call.is_map()) {
            source.push_str(&format!(
                "        let mut {map}: std::collections::HashMap<String, i64> = std::collections::HashMap::new();\n"
            ));
            for (key, value) in &self.entries {
                source.push_str(&format!(
                    "        {map}.insert({key:?}.to_string(), {value}i64);\n"
                ));
            }
        }
        for call in &self.calls {
            source.push_str(&format!(
                "        println!(\"method-{id}-{}:{{:?}}\", {});\n",
                call.name(),
                call.expression(&text, &values, &map, &self.needle, self.probe)
            ));
        }
        source.push_str("    }\n");
        source
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        for index in 0..self.calls.len() {
            if self.calls.len() > 1 {
                let mut candidate = self.clone();
                candidate.calls.remove(index);
                candidates.push(candidate);
            }
        }
        if !self.text.is_empty() {
            let mut candidate = self.clone();
            candidate.text = String::new();
            candidates.push(candidate);
        }
        if !self.values.is_empty() {
            let mut candidate = self.clone();
            candidate.values = Vec::new();
            candidates.push(candidate);
        }
        if !self.entries.is_empty() {
            let mut candidate = self.clone();
            candidate.entries = Vec::new();
            candidates.push(candidate);
        }
        candidates
    }

    pub fn shape(&self, output: &mut String) {
        output.push_str("methods[");
        for call in &self.calls {
            output.push_str(call.name());
            output.push(',');
        }
        output.push(']');
    }

    pub fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("methods");
        for call in &self.calls {
            output.insert(call.name());
        }
    }
}

const TEXTS: &[&str] = &[
    "",
    "rust script",
    "  Mixed CASE line  ",
    "λ two λ words",
    "line\nbreak\nthird",
    "aaa",
    " spaced ",
    "path/to/file.rs",
];

const NEEDLES: &[&str] = &["", "a", "rust", "λ", "line", " ", "/"];

pub fn generate_method_cases(rng: &mut StdRng) -> Vec<MethodsCase> {
    if !rng.random_bool(0.7) {
        return Vec::new();
    }
    let call_count = rng.random_range(5..=10);
    let mut calls = Vec::with_capacity(call_count);
    for _ in 0..call_count {
        calls.push(CATALOG[rng.random_range(0..CATALOG.len())]);
    }
    calls.dedup();
    let entry_count = rng.random_range(0..=4);
    let entries = (0..entry_count)
        .map(|_| {
            (
                NEEDLES[rng.random_range(0..NEEDLES.len())].to_string(),
                rng.random_range(-20..=20),
            )
        })
        .collect();
    let value_count = rng.random_range(0..=8);
    vec![MethodsCase {
        id: 0,
        text: TEXTS[rng.random_range(0..TEXTS.len())].to_string(),
        needle: NEEDLES[rng.random_range(0..NEEDLES.len())].to_string(),
        values: (0..value_count)
            .map(|_| rng.random_range(-20..=20))
            .collect(),
        probe: rng.random_range(-20..=20),
        entries,
        calls,
    }]
}
