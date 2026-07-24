use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::typed::{GeneratedExpr, GeneratedType};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GeneratedBinding {
    pub name: String,
    pub ty: GeneratedType,
    pub expr: GeneratedExpr,
    pub mutable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FlowStatement {
    Assign {
        target: String,
        value: GeneratedExpr,
    },
    IfAssign {
        target: String,
        condition: GeneratedExpr,
        then_value: GeneratedExpr,
        else_value: GeneratedExpr,
    },
    LoopAssign {
        target: String,
        index: String,
        iterations: usize,
        value: GeneratedExpr,
    },
}

impl FlowStatement {
    fn target(&self) -> &str {
        match self {
            Self::Assign { target, .. }
            | Self::IfAssign { target, .. }
            | Self::LoopAssign { target, .. } => target,
        }
    }

    fn uses(&self, name: &str) -> bool {
        match self {
            Self::Assign { value, .. } | Self::LoopAssign { value, .. } => value.uses(name),
            Self::IfAssign {
                condition,
                then_value,
                else_value,
                ..
            } => condition.uses(name) || then_value.uses(name) || else_value.uses(name),
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Assign { target, value } => {
                format!("        {target} = {};\n", value.render())
            }
            Self::IfAssign {
                target,
                condition,
                then_value,
                else_value,
            } => format!(
                "        if {} {{\n            {target} = {};\n        }} else {{\n            {target} = {};\n        }}\n",
                condition.render(),
                then_value.render(),
                else_value.render()
            ),
            Self::LoopAssign {
                target,
                index,
                iterations,
                value,
            } => format!(
                "        for {index} in 0i64..{iterations}i64 {{\n            {target} = {};\n        }}\n",
                value.render()
            ),
        }
    }

    fn shape(&self, output: &mut String) {
        match self {
            Self::Assign { value, .. } => {
                output.push_str("assign:");
                value.shape(output);
            }
            Self::IfAssign {
                condition,
                then_value,
                else_value,
                ..
            } => {
                output.push_str("if-assign:");
                condition.shape(output);
                then_value.shape(output);
                else_value.shape(output);
            }
            Self::LoopAssign { value, .. } => {
                output.push_str("loop-assign:");
                value.shape(output);
            }
        }
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        match self {
            Self::Assign { value, .. } => {
                output.insert("assignment");
                value.features(output);
            }
            Self::IfAssign {
                condition,
                then_value,
                else_value,
                ..
            } => {
                output.insert("if-statement");
                condition.features(output);
                then_value.features(output);
                else_value.features(output);
            }
            Self::LoopAssign { value, .. } => {
                output.insert("for-loop");
                value.features(output);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DataflowCase {
    pub id: usize,
    pub bindings: Vec<GeneratedBinding>,
    pub statements: Vec<FlowStatement>,
}

impl DataflowCase {
    fn render(&self) -> String {
        let mut source = String::from("    {\n");
        for binding in &self.bindings {
            let mutable = if binding.mutable { "mut " } else { "" };
            source.push_str(&format!(
                "        let {mutable}{}: {} = {};\n",
                binding.name,
                binding.ty.rust(),
                binding.expr.render()
            ));
        }
        for statement in &self.statements {
            source.push_str(&statement.render());
        }
        let values = self
            .bindings
            .iter()
            .map(|binding| binding.name.as_str())
            .collect::<Vec<_>>();
        let tuple = match values.as_slice() {
            [] => "()".to_string(),
            [value] => format!("({value},)"),
            _ => format!("({})", values.join(", ")),
        };
        source.push_str(&format!(
            "        println!(\"generated-dataflow-{}:{{:?}}\", {tuple});\n",
            self.id
        ));
        source.push_str("    }\n");
        source
    }

    fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if self.bindings.len() > 1 {
            let mut candidate = self.clone();
            if let Some(removed) = candidate.bindings.pop() {
                candidate.statements.retain(|statement| {
                    statement.target() != removed.name && !statement.uses(&removed.name)
                });
                candidates.push(candidate);
            }
        }
        for index in 0..self.statements.len() {
            let mut candidate = self.clone();
            candidate.statements.remove(index);
            candidates.push(candidate);
        }
        for (index, binding) in self.bindings.iter().enumerate() {
            for expression in binding.expr.shrinks() {
                let mut candidate = self.clone();
                candidate.bindings[index].expr = expression;
                candidates.push(candidate);
            }
        }
        candidates
    }

    fn shape(&self, output: &mut String) {
        output.push_str("dataflow[");
        for binding in &self.bindings {
            output.push_str(binding.ty.rust());
            binding.expr.shape(output);
        }
        for statement in &self.statements {
            statement.shape(output);
        }
        output.push(']');
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("dataflow");
        for binding in &self.bindings {
            output.insert(match binding.ty {
                GeneratedType::I64 => "type-i64",
                GeneratedType::F64 => "type-f64",
                GeneratedType::Bool => "type-bool",
                GeneratedType::String => "type-string",
                GeneratedType::VecI64 => "type-vec",
                GeneratedType::OptionI64 => "type-option",
            });
            binding.expr.features(output);
        }
        for statement in &self.statements {
            statement.features(output);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutableClosureKind {
    BorrowedMap,
    BorrowedLoop,
    OwnedFactory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutableClosureCase {
    pub id: usize,
    pub kind: MutableClosureKind,
    pub initial: i64,
    pub scale: i64,
    pub bias: i64,
    pub values: Vec<i64>,
    pub update: GeneratedExpr,
}

impl MutableClosureCase {
    fn render(&self) -> String {
        let state = format!("closure_state_{}", self.id);
        let item = format!("closure_item_{}", self.id);
        let scale = format!("closure_scale_{}", self.id);
        let bias = format!("closure_bias_{}", self.id);
        let input = format!("closure_input_{}", self.id);
        let operation = format!("closure_operation_{}", self.id);
        let output = format!("closure_output_{}", self.id);
        let values = i64_values(&self.values);
        let setup = format!(
            "    {{\n        let {scale} = {}i64;\n        let {bias} = {}i64;\n        let {input}: Vec<i64> = vec![{values}];\n",
            self.scale, self.bias
        );
        let update = self.update.render();
        match self.kind {
            MutableClosureKind::BorrowedMap => format!(
                r#"{setup}        let mut {state} = {initial}i64;
        let {output}: Vec<i64> = {{
            let mut {operation} = |{item}: i64| {{
                {state} = {update};
                {state}
            }};
            {input}
                .iter()
                .copied()
                .map(|{item}| {operation}({item}))
                .collect()
        }};
        println!("generated-closure-{id}:{{{state}}}:{{{output}:?}}");
    }}
"#,
                initial = self.initial,
                id = self.id
            ),
            MutableClosureKind::BorrowedLoop => format!(
                r#"{setup}        let mut {state} = {initial}i64;
        let mut {output}: Vec<i64> = Vec::new();
        {{
            let mut {operation} = |{item}: i64| {{
                {state} = {update};
                {state}
            }};
            for {item} in {input} {{
                {output}.push({operation}({item}));
            }}
        }}
        println!("generated-closure-{id}:{{{state}}}:{{{output}:?}}");
    }}
"#,
                initial = self.initial,
                id = self.id
            ),
            MutableClosureKind::OwnedFactory => {
                let factory = format!("closure_factory_{}", self.id);
                format!(
                    r#"{setup}        let {factory} = |mut {state}: i64| {{
            move |{item}: i64| {{
                {state} = {update};
                {state}
            }}
        }};
        let mut {operation} = {factory}({initial}i64);
        let {output}: Vec<i64> = {input}
            .into_iter()
            .map(|{item}| {operation}({item}))
            .collect();
        println!("generated-closure-{id}:{{{output}:?}}");
    }}
"#,
                    initial = self.initial,
                    id = self.id
                )
            }
        }
    }

    fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if self.values != [0] {
            let mut candidate = self.clone();
            candidate.values = vec![0];
            candidates.push(candidate);
        }
        if self.initial != 0 || self.scale != 1 || self.bias != 0 {
            let mut candidate = self.clone();
            candidate.initial = 0;
            candidate.scale = 1;
            candidate.bias = 0;
            candidates.push(candidate);
        }
        for update in self.update.shrinks() {
            let mut candidate = self.clone();
            candidate.update = update;
            candidates.push(candidate);
        }
        candidates
    }

    fn shape(&self, output: &mut String) {
        output.push_str(match self.kind {
            MutableClosureKind::BorrowedMap => "closure-borrowed-map:",
            MutableClosureKind::BorrowedLoop => "closure-borrowed-loop:",
            MutableClosureKind::OwnedFactory => "closure-owned-factory:",
        });
        self.update.shape(output);
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("mutable-closure");
        output.insert(match self.kind {
            MutableClosureKind::BorrowedMap => "closure-borrowed-map",
            MutableClosureKind::BorrowedLoop => "closure-borrowed-loop",
            MutableClosureKind::OwnedFactory => "closure-owned-factory",
        });
        self.update.features(output);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GeneratedEnumVariant {
    Unit,
    Number,
    Text,
    Pair,
    Values,
    MaybeSome,
    MaybeNone,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnumCase {
    pub id: usize,
    pub variant: GeneratedEnumVariant,
    pub number: i64,
    pub text: String,
    pub values: Vec<i64>,
    pub guard_length: usize,
    pub unit_arm: GeneratedExpr,
    pub number_arm: GeneratedExpr,
    pub text_guard_arm: GeneratedExpr,
    pub text_arm: GeneratedExpr,
    pub pair_arm: GeneratedExpr,
    pub values_arm: GeneratedExpr,
    pub some_arm: GeneratedExpr,
    pub none_arm: GeneratedExpr,
}

impl EnumCase {
    fn prelude(&self) -> String {
        format!(
            "enum GeneratedEnum{} {{\n    Unit,\n    Number(i64),\n    Text(String),\n    Pair {{ left: i64, right: i64 }},\n    Values(Vec<i64>),\n    Maybe(Option<i64>),\n}}\n\n",
            self.id
        )
    }

    fn render(&self) -> String {
        let enum_name = format!("GeneratedEnum{}", self.id);
        let state = match self.variant {
            GeneratedEnumVariant::Unit => format!("{enum_name}::Unit"),
            GeneratedEnumVariant::Number => format!("{enum_name}::Number({}i64)", self.number),
            GeneratedEnumVariant::Text => {
                format!("{enum_name}::Text({:?}.to_string())", self.text)
            }
            GeneratedEnumVariant::Pair => format!(
                "{enum_name}::Pair {{ left: {}i64, right: {}i64 }}",
                self.number,
                self.number.saturating_add(1)
            ),
            GeneratedEnumVariant::Values => {
                format!("{enum_name}::Values(vec![{}])", i64_values(&self.values))
            }
            GeneratedEnumVariant::MaybeSome => {
                format!("{enum_name}::Maybe(Some({}i64))", self.number)
            }
            GeneratedEnumVariant::MaybeNone => format!("{enum_name}::Maybe(None)"),
        };
        format!(
            r#"    {{
        let generated_state_{id} = {state};
        let generated_enum_output_{id} = match generated_state_{id} {{
            {enum_name}::Unit => {unit},
            {enum_name}::Number(enum_number_{id}) => {number},
            {enum_name}::Text(enum_text_{id}) if enum_text_{id}.len() > {guard}usize => {text_guard},
            {enum_name}::Text(enum_text_{id}) => {text},
            {enum_name}::Pair {{ left: enum_left_{id}, right: enum_right_{id} }} => {pair},
            {enum_name}::Values(enum_values_{id}) => {values},
            {enum_name}::Maybe(Some(enum_some_{id})) => {some},
            {enum_name}::Maybe(None) => {none},
        }};
        println!("generated-enum-{id}:{{generated_enum_output_{id}}}");
    }}
"#,
            id = self.id,
            guard = self.guard_length,
            unit = self.unit_arm.render(),
            number = self.number_arm.render(),
            text_guard = self.text_guard_arm.render(),
            text = self.text_arm.render(),
            pair = self.pair_arm.render(),
            values = self.values_arm.render(),
            some = self.some_arm.render(),
            none = self.none_arm.render()
        )
    }

    fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if self.variant != GeneratedEnumVariant::Unit
            || self.number != 0
            || !self.text.is_empty()
            || !self.values.is_empty()
        {
            let mut candidate = self.clone();
            candidate.variant = GeneratedEnumVariant::Unit;
            candidate.number = 0;
            candidate.text.clear();
            candidate.values.clear();
            candidates.push(candidate);
        }
        candidates
    }

    fn expressions(&self) -> [&GeneratedExpr; 8] {
        [
            &self.unit_arm,
            &self.number_arm,
            &self.text_guard_arm,
            &self.text_arm,
            &self.pair_arm,
            &self.values_arm,
            &self.some_arm,
            &self.none_arm,
        ]
    }

    fn shape(&self, output: &mut String) {
        output.push_str("enum-match:");
        for expression in self.expressions() {
            expression.shape(output);
        }
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("enum");
        output.insert("match-enum");
        output.insert("match-guard");
        output.insert("struct-pattern");
        output.insert("nested-option-pattern");
        for expression in self.expressions() {
            expression.features(output);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionParameter {
    pub name: String,
    pub ty: GeneratedType,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionCase {
    pub id: usize,
    pub parameters: Vec<FunctionParameter>,
    pub return_type: GeneratedType,
    pub body: GeneratedExpr,
    pub arguments: Vec<GeneratedExpr>,
    pub calls: usize,
}

impl FunctionCase {
    fn prelude(&self) -> String {
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| format!("{}: {}", parameter.name, parameter.ty.rust()))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "fn generated_function_{}({parameters}) -> {} {{\n    {}\n}}\n\n",
            self.id,
            self.return_type.rust(),
            self.body.render()
        )
    }

    fn render(&self) -> String {
        let arguments = self
            .arguments
            .iter()
            .map(GeneratedExpr::render)
            .collect::<Vec<_>>()
            .join(", ");
        let mut source = String::from("    {\n");
        for call in 0..self.calls {
            source.push_str(&format!(
                "        let generated_function_result_{}_{} = generated_function_{}({arguments});\n",
                self.id, call, self.id
            ));
            source.push_str(&format!(
                "        println!(\"generated-function-{}-{}:{{:?}}\", generated_function_result_{}_{});\n",
                self.id, call, self.id, call
            ));
        }
        source.push_str("    }\n");
        source
    }

    fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if self.calls > 1 {
            let mut candidate = self.clone();
            candidate.calls = 1;
            candidates.push(candidate);
        }
        for body in self.body.shrinks() {
            let mut candidate = self.clone();
            candidate.body = body;
            candidates.push(candidate);
        }
        candidates
    }

    fn shape(&self, output: &mut String) {
        output.push_str("function:");
        for parameter in &self.parameters {
            output.push_str(parameter.ty.rust());
        }
        self.body.shape(output);
        for argument in &self.arguments {
            argument.shape(output);
        }
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("function");
        self.body.features(output);
        for argument in &self.arguments {
            argument.features(output);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StructuralCase {
    Dataflow(DataflowCase),
    MutableClosure(MutableClosureCase),
    Enum(Box<EnumCase>),
    Function(FunctionCase),
}

impl StructuralCase {
    pub fn prelude(&self) -> String {
        match self {
            Self::Enum(case) => case.prelude(),
            Self::Function(case) => case.prelude(),
            Self::Dataflow(_) | Self::MutableClosure(_) => String::new(),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Dataflow(case) => case.render(),
            Self::MutableClosure(case) => case.render(),
            Self::Enum(case) => case.render(),
            Self::Function(case) => case.render(),
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        match self {
            Self::Dataflow(case) => case.shrinks().into_iter().map(Self::Dataflow).collect(),
            Self::MutableClosure(case) => case
                .shrinks()
                .into_iter()
                .map(Self::MutableClosure)
                .collect(),
            Self::Enum(case) => case
                .shrinks()
                .into_iter()
                .map(|case| Self::Enum(Box::new(case)))
                .collect(),
            Self::Function(case) => case.shrinks().into_iter().map(Self::Function).collect(),
        }
    }

    pub fn shape(&self, output: &mut String) {
        match self {
            Self::Dataflow(case) => case.shape(output),
            Self::MutableClosure(case) => case.shape(output),
            Self::Enum(case) => case.shape(output),
            Self::Function(case) => case.shape(output),
        }
    }

    pub fn features(&self, output: &mut BTreeSet<&'static str>) {
        match self {
            Self::Dataflow(case) => case.features(output),
            Self::MutableClosure(case) => case.features(output),
            Self::Enum(case) => case.features(output),
            Self::Function(case) => case.features(output),
        }
    }
}

fn i64_values(values: &[i64]) -> String {
    values
        .iter()
        .map(|value| format!("{value}i64"))
        .collect::<Vec<_>>()
        .join(", ")
}
