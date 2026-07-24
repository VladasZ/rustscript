use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::closure_case::{ClosureCase, apply_helper};
use crate::method_case::MethodsCase;
use crate::rich::RichCase;
use crate::semantic::SemanticCase;
use crate::structural::StructuralCase;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutationOperation {
    /// A same-typed donor subtree replaced a node in one of the parent's
    /// expression trees, free variables rebound to the target scope.
    Splice,
    CaseOrder,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationOrigin {
    pub parent_seed: u64,
    pub donor_seed: u64,
    pub operations: Vec<MutationOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ty {
    I64,
    Bool,
    String,
}

impl Ty {
    pub fn rust(self) -> &'static str {
        match self {
            Self::I64 => "i64",
            Self::Bool => "bool",
            Self::String => "String",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    I64(i64),
    Bool(bool),
    Text(String),
    Var {
        name: String,
        ty: Ty,
    },
    SaturatingAdd(Box<Expr>, Box<Expr>),
    SaturatingSub(Box<Expr>, Box<Expr>),
    SaturatingMul(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Less(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Concat(Box<Expr>, Box<Expr>),
    Repeat(Box<Expr>, usize),
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        ty: Ty,
    },
    Adjust {
        value: Box<Expr>,
        flag: Box<Expr>,
    },
}

impl Expr {
    pub fn ty(&self) -> Ty {
        match self {
            Self::I64(_)
            | Self::SaturatingAdd(..)
            | Self::SaturatingSub(..)
            | Self::SaturatingMul(..)
            | Self::Adjust { .. } => Ty::I64,
            Self::Bool(_)
            | Self::Eq(..)
            | Self::Less(..)
            | Self::And(..)
            | Self::Or(..)
            | Self::Not(..) => Ty::Bool,
            Self::Text(_) | Self::Concat(..) | Self::Repeat(..) => Ty::String,
            Self::Var { ty, .. } | Self::If { ty, .. } => *ty,
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::I64(value) => format!("{value}i64"),
            Self::Bool(value) => value.to_string(),
            Self::Text(value) => format!("String::from({value:?})"),
            Self::Var {
                name,
                ty: Ty::String,
            } => format!("{name}.clone()"),
            Self::Var { name, .. } => name.clone(),
            Self::SaturatingAdd(left, right) => {
                format!("{}.saturating_add({})", grouped(left), right.render())
            }
            Self::SaturatingSub(left, right) => {
                format!("{}.saturating_sub({})", grouped(left), right.render())
            }
            Self::SaturatingMul(left, right) => {
                format!("{}.saturating_mul({})", grouped(left), right.render())
            }
            Self::Eq(left, right) => format!("({} == {})", left.render(), right.render()),
            Self::Less(left, right) => format!("({} < {})", left.render(), right.render()),
            Self::And(left, right) => format!("({} && {})", left.render(), right.render()),
            Self::Or(left, right) => format!("({} || {})", left.render(), right.render()),
            Self::Not(value) => format!("!{}", grouped(value)),
            Self::Concat(left, right) => {
                format!(
                    "format!(\"{{}}{{}}\", {}, {})",
                    left.render(),
                    right.render()
                )
            }
            Self::Repeat(value, count) => format!("{}.repeat({count})", grouped(value)),
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => format!(
                "if {} {{ {} }} else {{ {} }}",
                condition.render(),
                then_expr.render(),
                else_expr.render()
            ),
            Self::Adjust { value, flag } => {
                format!("adjust({}, {})", value.render(), flag.render())
            }
        }
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Var { name, .. } => names.contains(name),
            Self::SaturatingAdd(left, right)
            | Self::SaturatingSub(left, right)
            | Self::SaturatingMul(left, right)
            | Self::Eq(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right) => left.uses_any(names) || right.uses_any(names),
            Self::Not(value) | Self::Repeat(value, _) => value.uses_any(names),
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.uses_any(names) || then_expr.uses_any(names) || else_expr.uses_any(names)
            }
            Self::Adjust { value, flag } => value.uses_any(names) || flag.uses_any(names),
            Self::I64(_) | Self::Bool(_) | Self::Text(_) => false,
        }
    }

    fn has_adjust(&self) -> bool {
        match self {
            Self::Adjust { .. } => true,
            Self::SaturatingAdd(left, right)
            | Self::SaturatingSub(left, right)
            | Self::SaturatingMul(left, right)
            | Self::Eq(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right) => left.has_adjust() || right.has_adjust(),
            Self::Not(value) | Self::Repeat(value, _) => value.has_adjust(),
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => condition.has_adjust() || then_expr.has_adjust() || else_expr.has_adjust(),
            Self::I64(_) | Self::Bool(_) | Self::Text(_) | Self::Var { .. } => false,
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        let minimal = minimal_expr(self.ty());
        if self != &minimal {
            candidates.push(minimal);
        }
        match self {
            Self::SaturatingAdd(left, right)
            | Self::SaturatingSub(left, right)
            | Self::SaturatingMul(left, right)
            | Self::Eq(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right) => {
                if left.ty() == self.ty() {
                    candidates.push((**left).clone());
                }
                if right.ty() == self.ty() {
                    candidates.push((**right).clone());
                }
            }
            Self::Not(value) | Self::Repeat(value, _) => {
                if value.ty() == self.ty() {
                    candidates.push((**value).clone());
                }
            }
            Self::If {
                then_expr,
                else_expr,
                ..
            } => {
                candidates.push((**then_expr).clone());
                candidates.push((**else_expr).clone());
            }
            Self::Adjust { value, .. } => candidates.push((**value).clone()),
            Self::I64(_) | Self::Bool(_) | Self::Text(_) | Self::Var { .. } => {}
        }
        candidates
    }
}

fn grouped(expr: &Expr) -> String {
    format!("({})", expr.render())
}

fn minimal_expr(ty: Ty) -> Expr {
    match ty {
        Ty::I64 => Expr::I64(0),
        Ty::Bool => Expr::Bool(false),
        Ty::String => Expr::Text(String::new()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        expr: Expr,
    },
    Assign {
        name: String,
        expr: Expr,
    },
    IfAssign {
        name: String,
        condition: Expr,
        then_expr: Expr,
        else_expr: Expr,
    },
    ForAdd {
        name: String,
        iterations: usize,
        delta: i64,
    },
}

impl Stmt {
    fn target(&self) -> &str {
        match self {
            Self::Let { name, .. }
            | Self::Assign { name, .. }
            | Self::IfAssign { name, .. }
            | Self::ForAdd { name, .. } => name,
        }
    }

    fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } => expr.uses_any(names),
            Self::IfAssign {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.uses_any(names) || then_expr.uses_any(names) || else_expr.uses_any(names)
            }
            Self::ForAdd { .. } => false,
        }
    }

    fn render(&self, mutable: &BTreeSet<String>) -> String {
        match self {
            Self::Let { name, ty, expr } => {
                let mutability = if mutable.contains(name) { "mut " } else { "" };
                format!(
                    "    let {mutability}{name}: {} = {};\n",
                    ty.rust(),
                    expr.render()
                )
            }
            Self::Assign { name, expr } => {
                format!("    {name} = {};\n", expr.render())
            }
            Self::IfAssign {
                name,
                condition,
                then_expr,
                else_expr,
            } => format!(
                "    if {} {{\n        {name} = {};\n    }} else {{\n        {name} = {};\n    }}\n",
                condition.render(),
                then_expr.render(),
                else_expr.render()
            ),
            Self::ForAdd {
                name,
                iterations,
                delta,
            } => format!(
                "    for index_{name} in 0i64..{iterations}i64 {{\n        {name} = {name}.saturating_add({delta}i64.saturating_add(index_{name}));\n    }}\n"
            ),
        }
    }

    fn shrinks(&self) -> Vec<Self> {
        match self {
            Self::Let { name, ty, expr } => expr
                .shrinks()
                .into_iter()
                .map(|expr| Self::Let {
                    name: name.clone(),
                    ty: *ty,
                    expr,
                })
                .collect(),
            Self::Assign { name, expr } => expr
                .shrinks()
                .into_iter()
                .map(|expr| Self::Assign {
                    name: name.clone(),
                    expr,
                })
                .collect(),
            Self::IfAssign {
                name,
                condition,
                then_expr,
                else_expr,
            } => {
                let mut candidates = vec![
                    Self::Assign {
                        name: name.clone(),
                        expr: then_expr.clone(),
                    },
                    Self::Assign {
                        name: name.clone(),
                        expr: else_expr.clone(),
                    },
                ];
                for condition in condition.shrinks() {
                    candidates.push(Self::IfAssign {
                        name: name.clone(),
                        condition,
                        then_expr: then_expr.clone(),
                        else_expr: else_expr.clone(),
                    });
                }
                candidates
            }
            Self::ForAdd {
                name,
                iterations,
                delta,
            } => {
                let mut candidates = Vec::new();
                if *iterations != 1 {
                    candidates.push(Self::ForAdd {
                        name: name.clone(),
                        iterations: 1,
                        delta: *delta,
                    });
                }
                if *delta != 0 {
                    candidates.push(Self::ForAdd {
                        name: name.clone(),
                        iterations: *iterations,
                        delta: 0,
                    });
                }
                candidates
            }
        }
    }

    fn has_adjust(&self) -> bool {
        match self {
            Self::Let { expr, .. } | Self::Assign { expr, .. } => expr.has_adjust(),
            Self::IfAssign {
                condition,
                then_expr,
                else_expr,
                ..
            } => condition.has_adjust() || then_expr.has_adjust() || else_expr.has_adjust(),
            Self::ForAdd { .. } => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub seed: u64,
    pub adjustment: i64,
    pub statements: Vec<Stmt>,
    #[serde(default)]
    pub rich_cases: Vec<RichCase>,
    #[serde(default)]
    pub closure_cases: Vec<ClosureCase>,
    #[serde(default)]
    pub structural_cases: Vec<StructuralCase>,
    #[serde(default)]
    pub semantic_cases: Vec<SemanticCase>,
    #[serde(default)]
    pub method_cases: Vec<MethodsCase>,
    #[serde(default)]
    pub mutation: Option<MutationOrigin>,
}

impl Program {
    pub fn render(&self) -> String {
        let mutable: BTreeSet<String> = self
            .statements
            .iter()
            .filter_map(|statement| match statement {
                Stmt::Assign { name, .. }
                | Stmt::IfAssign { name, .. }
                | Stmt::ForAdd { name, .. } => Some(name.clone()),
                Stmt::Let { .. } => None,
            })
            .collect();
        let mut source = format!(
            "// generated by rustscript-differential, seed {}\n\n",
            self.seed
        );
        if let Some(origin) = &self.mutation {
            source.push_str(&format!(
                "// structured mutation from seeds {} and {} using {:?}\n\n",
                origin.parent_seed, origin.donor_seed, origin.operations
            ));
        }
        if self.rich_cases.iter().any(RichCase::requires_path) {
            source.push_str("use std::path::PathBuf;\n\n");
        }
        if self
            .rich_cases
            .iter()
            .any(RichCase::requires_generated_state)
        {
            source.push_str(
                "enum GeneratedState {\n    Idle,\n    Named(String),\n    Located { path: PathBuf, values: Vec<i64> },\n}\n\n",
            );
        }
        if self
            .closure_cases
            .iter()
            .any(ClosureCase::requires_apply_helper)
        {
            source.push_str(apply_helper());
        }
        if self.structural_features().iter().any(|feature| {
            feature.starts_with("raw-")
                || *feature == "numeric-opaque"
                || *feature == "numeric-shift-oversized"
        }) {
            source.push_str(crate::typed::opaque_helper());
        }
        for structural_case in &self.structural_cases {
            source.push_str(&structural_case.prelude());
        }
        for semantic_case in &self.semantic_cases {
            source.push_str(&semantic_case.prelude());
        }
        if self.statements.iter().any(Stmt::has_adjust) {
            source.push_str(&format!(
                "fn adjust(value: i64, flag: bool) -> i64 {{\n    if flag {{\n        value + {}i64\n    }} else {{\n        value - {}i64\n    }}\n}}\n\n",
                self.adjustment, self.adjustment
            ));
        }
        source.push_str("fn main() {\n");
        for statement in &self.statements {
            source.push_str(&statement.render(&mutable));
        }
        for rich_case in &self.rich_cases {
            source.push_str(&rich_case.render());
        }
        for closure_case in &self.closure_cases {
            source.push_str(&closure_case.render());
        }
        for structural_case in &self.structural_cases {
            source.push_str(&structural_case.render());
        }
        for semantic_case in &self.semantic_cases {
            source.push_str(&semantic_case.render());
        }
        for method_case in &self.method_cases {
            source.push_str(&method_case.render());
        }
        let names: Vec<&str> = self
            .statements
            .iter()
            .filter_map(|statement| match statement {
                Stmt::Let { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        source.push_str("    println!(\"{:?}\", ");
        match names.as_slice() {
            [] => source.push_str("()"),
            [name] => source.push_str(&format!("({name},)")),
            _ => source.push_str(&format!("({})", names.join(", "))),
        }
        source.push_str(");\n}\n");
        source
    }

    pub fn shrink_candidates(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        if self.adjustment != 0 {
            let mut candidate = self.clone();
            candidate.adjustment = 0;
            candidates.push(candidate);
        }
        for index in 0..self.statements.len() {
            if matches!(self.statements[index], Stmt::Let { .. }) {
                if let Some(candidate) = self.without_binding(index) {
                    candidates.push(candidate);
                }
            } else {
                let mut candidate = self.clone();
                candidate.statements.remove(index);
                candidates.push(candidate);
            }
            for statement in self.statements[index].shrinks() {
                let mut candidate = self.clone();
                candidate.statements[index] = statement;
                candidates.push(candidate);
            }
        }
        for index in 0..self.rich_cases.len() {
            let mut candidate = self.clone();
            candidate.rich_cases.remove(index);
            candidates.push(candidate);
            for rich_case in self.rich_cases[index].shrinks() {
                let mut candidate = self.clone();
                candidate.rich_cases[index] = rich_case;
                candidates.push(candidate);
            }
        }
        for index in 0..self.closure_cases.len() {
            let mut candidate = self.clone();
            candidate.closure_cases.remove(index);
            candidates.push(candidate);
            for closure_case in self.closure_cases[index].shrinks() {
                let mut candidate = self.clone();
                candidate.closure_cases[index] = closure_case;
                candidates.push(candidate);
            }
        }
        for index in 0..self.structural_cases.len() {
            let mut candidate = self.clone();
            candidate.structural_cases.remove(index);
            candidates.push(candidate);
            for structural_case in self.structural_cases[index].shrinks() {
                let mut candidate = self.clone();
                candidate.structural_cases[index] = structural_case;
                candidates.push(candidate);
            }
        }
        for index in 0..self.semantic_cases.len() {
            let mut candidate = self.clone();
            candidate.semantic_cases.remove(index);
            candidates.push(candidate);
            for semantic_case in self.semantic_cases[index].shrinks() {
                let mut candidate = self.clone();
                candidate.semantic_cases[index] = semantic_case;
                candidates.push(candidate);
            }
        }
        for index in 0..self.method_cases.len() {
            let mut candidate = self.clone();
            candidate.method_cases.remove(index);
            candidates.push(candidate);
            for method_case in self.method_cases[index].shrinks() {
                let mut candidate = self.clone();
                candidate.method_cases[index] = method_case;
                candidates.push(candidate);
            }
        }
        candidates
    }

    pub fn structural_signature(&self) -> String {
        let mut signature = String::new();
        signature.push_str("statements[");
        for statement in &self.statements {
            signature.push_str(match statement {
                Stmt::Let { .. } => "let,",
                Stmt::Assign { .. } => "assign,",
                Stmt::IfAssign { .. } => "if,",
                Stmt::ForAdd { .. } => "for,",
            });
        }
        signature.push_str("]rich[");
        for rich_case in &self.rich_cases {
            signature.push_str(match rich_case {
                RichCase::OptionClosure { .. } => "option,",
                RichCase::VectorPipeline { .. } => "vector,",
                RichCase::PathString { .. } => "path,",
                RichCase::EnumMatch { .. } => "enum,",
            });
        }
        signature.push_str("]closures[");
        for closure_case in &self.closure_cases {
            signature.push_str(match closure_case {
                ClosureCase::Nested { .. } => "nested,",
                ClosureCase::MutableCapture { .. } => "mutable,",
                ClosureCase::MoveString { .. } => "move,",
                ClosureCase::CapturedCall { .. } => "captured,",
                ClosureCase::TuplePattern { .. } => "tuple,",
                ClosureCase::GenericApply { .. } => "generic,",
            });
        }
        signature.push_str("]generated[");
        for structural_case in &self.structural_cases {
            structural_case.shape(&mut signature);
            signature.push('|');
        }
        for semantic_case in &self.semantic_cases {
            semantic_case.shape(&mut signature);
            signature.push('|');
        }
        for method_case in &self.method_cases {
            method_case.shape(&mut signature);
            signature.push('|');
        }
        signature.push(']');
        if let Some(origin) = &self.mutation {
            signature.push_str("mutation[");
            for operation in &origin.operations {
                signature.push_str(match operation {
                    MutationOperation::Splice => "splice,",
                    MutationOperation::CaseOrder => "order,",
                });
            }
            signature.push(']');
        }
        signature
    }

    pub fn structural_features(&self) -> BTreeSet<&'static str> {
        let mut features = BTreeSet::new();
        for structural_case in &self.structural_cases {
            structural_case.features(&mut features);
        }
        for semantic_case in &self.semantic_cases {
            semantic_case.features(&mut features);
        }
        for method_case in &self.method_cases {
            method_case.features(&mut features);
        }
        features
    }

    fn without_binding(&self, index: usize) -> Option<Self> {
        let Stmt::Let { name, .. } = &self.statements[index] else {
            return None;
        };
        let mut removed = BTreeSet::from([name.clone()]);
        let mut statements = Vec::new();
        for statement in &self.statements {
            if removed.contains(statement.target()) || statement.uses_any(&removed) {
                if let Stmt::Let { name, .. } = statement {
                    removed.insert(name.clone());
                }
                continue;
            }
            statements.push(statement.clone());
        }
        Some(Self {
            seed: self.seed,
            adjustment: self.adjustment,
            statements,
            rich_cases: self.rich_cases.clone(),
            closure_cases: self.closure_cases.clone(),
            structural_cases: self.structural_cases.clone(),
            semantic_cases: self.semantic_cases.clone(),
            method_cases: self.method_cases.clone(),
            mutation: self.mutation.clone(),
        })
    }
}
