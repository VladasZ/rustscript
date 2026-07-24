use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Every integer width real Rust has, including u64 and usize whose full
/// range does not fit the i64 the interpreter computes in. The grammar covers
/// what the language supports, never only what the interpreter handles. A
/// divergence found here goes to the quarantine list, not out of the grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntWidth {
    U8,
    U16,
    U32,
    U64,
    USize,
    I8,
    I16,
    I32,
    I64,
}

impl IntWidth {
    pub(crate) fn rust(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::USize => "usize",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
        }
    }

    fn feature(self) -> &'static str {
        match self {
            Self::U8 => "width-u8",
            Self::U16 => "width-u16",
            Self::U32 => "width-u32",
            Self::U64 => "width-u64",
            Self::USize => "width-usize",
            Self::I8 => "width-i8",
            Self::I16 => "width-i16",
            Self::I32 => "width-i32",
            Self::I64 => "width-i64",
        }
    }

    pub(crate) fn is_signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    pub(crate) fn bits(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 8,
            Self::U16 | Self::I16 => 16,
            Self::U32 | Self::I32 => 32,
            Self::U64 | Self::USize | Self::I64 => 64,
        }
    }

    /// The harness only runs on 64-bit targets, so usize takes u64's range.
    pub(crate) fn min(self) -> i128 {
        if self.is_signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    pub(crate) fn max(self) -> i128 {
        if self.is_signed() {
            (1i128 << (self.bits() - 1)) - 1
        } else {
            (1i128 << self.bits()) - 1
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FloatWidth {
    F32,
    F64,
}

impl FloatWidth {
    pub(crate) fn rust(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    fn feature(self) -> &'static str {
        match self {
            Self::F32 => "width-f32",
            Self::F64 => "width-f64",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl IntOp {
    fn token(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
        }
    }

    fn feature(self) -> &'static str {
        match self {
            Self::Add => "numeric-add",
            Self::Sub => "numeric-sub",
            Self::Mul => "numeric-mul",
            Self::Div => "numeric-div",
            Self::Rem => "numeric-rem",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FloatOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl FloatOp {
    fn token(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ShiftDirection {
    Left,
    Right,
}

impl ShiftDirection {
    fn token(self) -> &'static str {
        match self {
            Self::Left => "<<",
            Self::Right => ">>",
        }
    }
}

/// One side of a binary integer statement. A literal is rendered bare, so its
/// width comes from inference against the variable on the other side.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntOperand {
    Var(String),
    Literal(i128),
}

impl IntOperand {
    fn render(&self) -> String {
        match self {
            Self::Var(name) => name.clone(),
            Self::Literal(value) if *value < 0 => format!("({value})"),
            Self::Literal(value) => value.to_string(),
        }
    }

    fn uses(&self, name: &str) -> bool {
        matches!(self, Self::Var(variable) if variable == name)
    }
}

/// One side of a binary float statement. Tokens are rendered verbatim, plain
/// ones like `0.25` take their width from the variable on the other side and
/// constants like `f32::NAN` carry their own.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FloatOperand {
    Var(String),
    Token(String),
}

impl FloatOperand {
    fn render(&self) -> String {
        match self {
            Self::Var(name) => name.clone(),
            Self::Token(token) => token.clone(),
        }
    }

    fn uses(&self, name: &str) -> bool {
        matches!(self, Self::Var(variable) if variable == name)
    }
}

/// Numeric statements hunt the divergences an interpreter with one internal
/// integer and one internal float type produces. The width of a value flows
/// through bindings, inference, and casts across statements, the shapes a
/// syntactic per-expression guard cannot see. Values that must stay out of
/// the compiler's constant folding pass through `diff_opaque`, so a panic
/// stays a runtime event instead of a compile error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NumericStatement {
    /// `let name: u8 = 255;` The width lives only in the annotation.
    LetAnnotated {
        name: String,
        width: IntWidth,
        value: i128,
    },
    /// `let name = 255u8;` The width lives only in the literal suffix.
    LetSuffixed {
        name: String,
        width: IntWidth,
        value: i128,
    },
    /// `let name = diff_opaque(value) as u8;` The only integer source the
    /// compiler cannot see through.
    LetOpaque {
        name: String,
        width: IntWidth,
        value: i64,
    },
    /// `let name = left op right;` The width is never written down, it comes
    /// from the operands by inference.
    LetBinary {
        name: String,
        width: IntWidth,
        op: IntOp,
        left: String,
        right: IntOperand,
    },
    /// `target op= operand;`
    Compound {
        target: String,
        op: IntOp,
        operand: IntOperand,
    },
    /// `let name = source << amount;` An oversized amount panics in debug
    /// Rust and goes through the opaque helper because the overflow lint
    /// folds a plain literal amount.
    Shift {
        name: String,
        width: IntWidth,
        source: String,
        direction: ShiftDirection,
        amount: u32,
        oversized: bool,
    },
    /// `let name = -source;` Panics on the minimum of a signed width.
    Negate {
        name: String,
        width: IntWidth,
        source: String,
    },
    /// `let name = source as u16;` A cast whose input was narrowed in an
    /// earlier statement.
    Recast {
        name: String,
        width: IntWidth,
        source: String,
    },
    /// `let name: f32 = 0.1;`
    FloatLet {
        name: String,
        width: FloatWidth,
        token: String,
    },
    /// `let name = left op right;` over floats. Never panics, but f32 kept
    /// internally as f64 changes results and printed strings.
    FloatBinary {
        name: String,
        width: FloatWidth,
        op: FloatOp,
        left: String,
        right: FloatOperand,
    },
    /// `let name = source as f32;`
    IntToFloat {
        name: String,
        width: FloatWidth,
        source: String,
    },
    /// `let name = source as i8;` Saturates and maps NaN to zero in real
    /// Rust.
    FloatToInt {
        name: String,
        width: IntWidth,
        source: String,
    },
}

impl NumericStatement {
    pub(crate) fn declared(&self) -> Option<&str> {
        match self {
            Self::LetAnnotated { name, .. }
            | Self::LetSuffixed { name, .. }
            | Self::LetOpaque { name, .. }
            | Self::LetBinary { name, .. }
            | Self::Shift { name, .. }
            | Self::Negate { name, .. }
            | Self::Recast { name, .. }
            | Self::FloatLet { name, .. }
            | Self::FloatBinary { name, .. }
            | Self::IntToFloat { name, .. }
            | Self::FloatToInt { name, .. } => Some(name),
            Self::Compound { .. } => None,
        }
    }

    fn declares_float(&self) -> bool {
        matches!(
            self,
            Self::FloatLet { .. } | Self::FloatBinary { .. } | Self::IntToFloat { .. }
        )
    }

    fn uses(&self, name: &str) -> bool {
        match self {
            Self::LetAnnotated { .. } | Self::LetSuffixed { .. } | Self::LetOpaque { .. } => false,
            Self::LetBinary { left, right, .. } => left == name || right.uses(name),
            Self::Compound {
                target, operand, ..
            } => target == name || operand.uses(name),
            Self::Shift { source, .. }
            | Self::Negate { source, .. }
            | Self::Recast { source, .. }
            | Self::IntToFloat { source, .. }
            | Self::FloatToInt { source, .. } => source == name,
            Self::FloatLet { .. } => false,
            Self::FloatBinary { left, right, .. } => left == name || right.uses(name),
        }
    }

    fn render(&self, mutable: &BTreeSet<String>) -> String {
        match self {
            Self::LetAnnotated { name, width, value } => format!(
                "        let {}: {} = {};\n",
                binding(name, mutable),
                width.rust(),
                int_token(*value, *width, false)
            ),
            Self::LetSuffixed { name, width, value } => format!(
                "        let {} = {};\n",
                binding(name, mutable),
                int_token(*value, *width, true)
            ),
            Self::LetOpaque { name, width, value } => format!(
                "        let {} = diff_opaque({}) as {};\n",
                binding(name, mutable),
                i64_token(*value),
                width.rust()
            ),
            Self::LetBinary {
                name,
                op,
                left,
                right,
                ..
            } => format!(
                "        let {} = {left} {} {};\n",
                binding(name, mutable),
                op.token(),
                right.render()
            ),
            Self::Compound {
                target,
                op,
                operand,
            } => format!("        {target} {}= {};\n", op.token(), operand.render()),
            Self::Shift {
                name,
                source,
                direction,
                amount,
                oversized,
                ..
            } => {
                if *oversized {
                    format!(
                        "        let {} = {source} {} (diff_opaque({amount}i64) as u32);\n",
                        binding(name, mutable),
                        direction.token()
                    )
                } else {
                    format!(
                        "        let {} = {source} {} {amount};\n",
                        binding(name, mutable),
                        direction.token()
                    )
                }
            }
            Self::Negate { name, source, .. } => {
                format!("        let {} = -{source};\n", binding(name, mutable))
            }
            Self::Recast {
                name,
                width,
                source,
            } => format!(
                "        let {} = {source} as {};\n",
                binding(name, mutable),
                width.rust()
            ),
            Self::FloatLet { name, width, token } => format!(
                "        let {}: {} = {token};\n",
                binding(name, mutable),
                width.rust()
            ),
            Self::FloatBinary {
                name,
                op,
                left,
                right,
                ..
            } => format!(
                "        let {} = {left} {} {};\n",
                binding(name, mutable),
                op.token(),
                right.render()
            ),
            Self::IntToFloat {
                name,
                width,
                source,
            } => format!(
                "        let {} = {source} as {};\n",
                binding(name, mutable),
                width.rust()
            ),
            Self::FloatToInt {
                name,
                width,
                source,
            } => format!(
                "        let {} = {source} as {};\n",
                binding(name, mutable),
                width.rust()
            ),
        }
    }

    fn tag(&self) -> &'static str {
        match self {
            Self::LetAnnotated { .. } => "annotated",
            Self::LetSuffixed { .. } => "suffixed",
            Self::LetOpaque { .. } => "opaque",
            Self::LetBinary { .. } => "binary",
            Self::Compound { .. } => "compound",
            Self::Shift { .. } => "shift",
            Self::Negate { .. } => "negate",
            Self::Recast { .. } => "recast",
            Self::FloatLet { .. } => "float-let",
            Self::FloatBinary { .. } => "float-binary",
            Self::IntToFloat { .. } => "int-to-float",
            Self::FloatToInt { .. } => "float-to-int",
        }
    }

    fn width_name(&self) -> &'static str {
        match self {
            Self::LetAnnotated { width, .. }
            | Self::LetSuffixed { width, .. }
            | Self::LetOpaque { width, .. }
            | Self::LetBinary { width, .. }
            | Self::Shift { width, .. }
            | Self::Negate { width, .. }
            | Self::Recast { width, .. }
            | Self::FloatToInt { width, .. } => width.rust(),
            Self::FloatLet { width, .. }
            | Self::FloatBinary { width, .. }
            | Self::IntToFloat { width, .. } => width.rust(),
            Self::Compound { .. } => "",
        }
    }

    fn features(&self, output: &mut BTreeSet<&'static str>) {
        match self {
            Self::LetAnnotated { width, .. } => {
                output.insert("numeric-let-annotated");
                output.insert(width.feature());
            }
            Self::LetSuffixed { width, .. } => {
                output.insert("numeric-let-suffixed");
                output.insert(width.feature());
            }
            Self::LetOpaque { width, .. } => {
                output.insert("numeric-opaque");
                output.insert(width.feature());
            }
            Self::LetBinary { width, op, .. } => {
                output.insert("numeric-binary");
                output.insert(op.feature());
                output.insert(width.feature());
            }
            Self::Compound { op, .. } => {
                output.insert("numeric-compound");
                output.insert(op.feature());
            }
            Self::Shift {
                width, oversized, ..
            } => {
                output.insert("numeric-shift");
                output.insert(width.feature());
                if *oversized {
                    output.insert("numeric-shift-oversized");
                }
            }
            Self::Negate { width, .. } => {
                output.insert("numeric-negate");
                output.insert(width.feature());
            }
            Self::Recast { width, .. } => {
                output.insert("numeric-recast");
                output.insert(width.feature());
            }
            Self::FloatLet { width, .. } => {
                output.insert("numeric-float-let");
                output.insert(width.feature());
            }
            Self::FloatBinary { width, .. } => {
                output.insert("numeric-float-binary");
                output.insert(width.feature());
            }
            Self::IntToFloat { width, .. } => {
                output.insert("numeric-int-to-float");
                output.insert(width.feature());
            }
            Self::FloatToInt { width, .. } => {
                output.insert("numeric-float-to-int");
                output.insert(width.feature());
            }
        }
    }

    /// Smaller values for the minimizer. Division and remainder literals stay
    /// off zero so a shrink candidate cannot become a constant divide by zero
    /// the compiler rejects.
    fn simplify(&self) -> Vec<Self> {
        let mut simplified = Vec::new();
        match self {
            Self::LetAnnotated { name, width, value } => {
                for smaller in shrink_value(*value) {
                    simplified.push(Self::LetAnnotated {
                        name: name.clone(),
                        width: *width,
                        value: smaller,
                    });
                }
            }
            Self::LetSuffixed { name, width, value } => {
                for smaller in shrink_value(*value) {
                    simplified.push(Self::LetSuffixed {
                        name: name.clone(),
                        width: *width,
                        value: smaller,
                    });
                }
            }
            Self::LetOpaque { name, width, value } => {
                for smaller in shrink_value(i128::from(*value)) {
                    simplified.push(Self::LetOpaque {
                        name: name.clone(),
                        width: *width,
                        value: smaller as i64,
                    });
                }
            }
            Self::LetBinary {
                name,
                width,
                op,
                left,
                right: IntOperand::Literal(value),
            } => {
                let floor = if matches!(op, IntOp::Div | IntOp::Rem) {
                    1
                } else {
                    0
                };
                for smaller in shrink_value(*value) {
                    if smaller.abs() < floor {
                        continue;
                    }
                    simplified.push(Self::LetBinary {
                        name: name.clone(),
                        width: *width,
                        op: *op,
                        left: left.clone(),
                        right: IntOperand::Literal(smaller),
                    });
                }
            }
            Self::Shift {
                name,
                width,
                source,
                direction,
                amount,
                oversized,
            } if *amount > 0 => {
                simplified.push(Self::Shift {
                    name: name.clone(),
                    width: *width,
                    source: source.clone(),
                    direction: *direction,
                    amount: amount / 2,
                    oversized: *oversized,
                });
            }
            _ => {}
        }
        simplified
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NumericCase {
    pub id: usize,
    pub statements: Vec<NumericStatement>,
}

impl NumericCase {
    pub(crate) fn render(&self) -> String {
        let mutable: BTreeSet<String> = self
            .statements
            .iter()
            .filter_map(|statement| match statement {
                NumericStatement::Compound { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        let mut source = String::from("    {\n");
        for statement in &self.statements {
            source.push_str(&statement.render(&mutable));
        }
        for (index, statement) in self.statements.iter().enumerate() {
            let Some(name) = statement.declared() else {
                continue;
            };
            source.push_str(&format!(
                "        println!(\"generated-numeric-{}-{index}:{{:?}}\", {name});\n",
                self.id
            ));
            if statement.declares_float() {
                source.push_str(&format!(
                    "        println!(\"generated-numeric-{}-{index}-display:{{}}\", {name});\n",
                    self.id
                ));
            }
        }
        source.push_str("    }\n");
        source
    }

    pub(crate) fn shrinks(&self) -> Vec<Self> {
        let mut candidates = Vec::new();
        for index in 0..self.statements.len() {
            let statement = &self.statements[index];
            let removable = match statement.declared() {
                Some(name) => !self.statements[index + 1..]
                    .iter()
                    .any(|later| later.uses(name)),
                None => true,
            };
            if removable && self.statements.len() > 1 {
                let mut candidate = self.clone();
                candidate.statements.remove(index);
                candidates.push(candidate);
            }
            for simplified in statement.simplify() {
                let mut candidate = self.clone();
                candidate.statements[index] = simplified;
                candidates.push(candidate);
            }
        }
        candidates
    }

    pub(crate) fn shape(&self, output: &mut String) {
        output.push_str("numeric[");
        for statement in &self.statements {
            output.push_str(statement.tag());
            output.push(':');
            output.push_str(statement.width_name());
            output.push(',');
        }
        output.push(']');
    }

    pub(crate) fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert("numeric");
        for statement in &self.statements {
            statement.features(output);
        }
    }
}

fn binding(name: &str, mutable: &BTreeSet<String>) -> String {
    if mutable.contains(name) {
        format!("mut {name}")
    } else {
        name.to_string()
    }
}

/// The 64-bit extremes cannot be written as plain literals, the minimum
/// overflows before negation and the unsigned maximum overflows an annotated
/// 32-bit build, so those render through their associated constants.
fn int_token(value: i128, width: IntWidth, suffixed: bool) -> String {
    let const_min = width == IntWidth::I64 && value == width.min();
    let const_max = matches!(width, IntWidth::U64 | IntWidth::USize) && value == width.max();
    if const_min {
        format!("{}::MIN", width.rust())
    } else if const_max {
        format!("{}::MAX", width.rust())
    } else if suffixed {
        format!("{value}{}", width.rust())
    } else {
        value.to_string()
    }
}

fn i64_token(value: i64) -> String {
    if value == i64::MIN {
        "i64::MIN".to_string()
    } else {
        format!("{value}i64")
    }
}

fn shrink_value(value: i128) -> Vec<i128> {
    let mut smaller = Vec::new();
    if value != 0 {
        smaller.push(0);
    }
    if value / 2 != value && value / 2 != 0 {
        smaller.push(value / 2);
    }
    smaller
}
