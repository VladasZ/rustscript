use rand::RngExt;
use rand::rngs::StdRng;

use crate::numeric::{
    FloatOp, FloatOperand, FloatWidth, IntOp, IntOperand, IntWidth, NumericCase, NumericStatement,
    ShiftDirection,
};

/// A bound integer variable. `opaque` means the compiler cannot fold its
/// value, because it came through `diff_opaque` or from an operation with an
/// opaque operand. Every statement that can panic keeps at least one opaque
/// operand, so the overflow lint never rejects the program and the panic
/// stays a runtime event, which is the behavior under test.
struct IntVar {
    name: String,
    width: IntWidth,
    opaque: bool,
}

struct FloatVar {
    name: String,
    width: FloatWidth,
}

pub fn generate_numeric(id: usize, rng: &mut StdRng) -> NumericCase {
    let mut builder = Builder {
        id,
        statements: Vec::new(),
        ints: Vec::new(),
        floats: Vec::new(),
        next: 0,
    };
    let seeds = rng.random_range(2..=4);
    for _ in 0..seeds {
        builder.seed(rng);
    }
    let derived = rng.random_range(5..=10);
    for _ in 0..derived {
        builder.derived(rng);
    }
    NumericCase {
        id,
        statements: builder.statements,
    }
}

struct Builder {
    id: usize,
    statements: Vec<NumericStatement>,
    ints: Vec<IntVar>,
    floats: Vec<FloatVar>,
    next: usize,
}

impl Builder {
    fn fresh(&mut self) -> String {
        let name = format!("num_{}_{}", self.id, self.next);
        self.next += 1;
        name
    }

    fn seed(&mut self, rng: &mut StdRng) {
        let width = random_width(rng);
        match rng.random_range(0..3) {
            0 => {
                let name = self.fresh();
                self.statements.push(NumericStatement::LetAnnotated {
                    name: name.clone(),
                    width,
                    value: wild_value(width, rng),
                });
                self.ints.push(IntVar {
                    name,
                    width,
                    opaque: false,
                });
            }
            1 => {
                let name = self.fresh();
                self.statements.push(NumericStatement::LetSuffixed {
                    name: name.clone(),
                    width,
                    value: wild_value(width, rng),
                });
                self.ints.push(IntVar {
                    name,
                    width,
                    opaque: false,
                });
            }
            _ => {
                self.push_opaque(width, rng);
            }
        }
    }

    /// A variable the compiler cannot fold, seeded with a value that lands on
    /// or past the width's boundaries after the cast.
    fn push_opaque(&mut self, width: IntWidth, rng: &mut StdRng) -> String {
        let name = self.fresh();
        self.statements.push(NumericStatement::LetOpaque {
            name: name.clone(),
            width,
            value: wild_opaque(width, rng),
        });
        self.ints.push(IntVar {
            name: name.clone(),
            width,
            opaque: true,
        });
        name
    }

    fn opaque_of(&mut self, width: IntWidth, rng: &mut StdRng) -> String {
        let candidates: Vec<&IntVar> = self
            .ints
            .iter()
            .filter(|var| var.width == width && var.opaque)
            .collect();
        if candidates.is_empty() {
            return self.push_opaque(width, rng);
        }
        candidates[rng.random_range(0..candidates.len())]
            .name
            .clone()
    }

    fn pick_int(&self, rng: &mut StdRng) -> usize {
        rng.random_range(0..self.ints.len())
    }

    fn derived(&mut self, rng: &mut StdRng) {
        match rng.random_range(0..12) {
            0 => self.seed(rng),
            1..=3 => self.binary(rng),
            4 => self.compound(rng),
            5 => self.shift(rng),
            6 => self.negate(rng),
            7 => self.recast(rng),
            8 => {
                self.float_let(rng);
            }
            9 => self.float_binary(rng),
            10 => self.int_to_float(rng),
            _ => self.float_to_int(rng),
        }
    }

    fn binary(&mut self, rng: &mut StdRng) {
        let picked = self.pick_int(rng);
        let width = self.ints[picked].width;
        let left_opaque = self.ints[picked].opaque;
        let mut left = self.ints[picked].name.clone();
        let op = random_op(rng);
        let right = if rng.random_bool(0.5) {
            if left_opaque {
                let same: Vec<&IntVar> =
                    self.ints.iter().filter(|var| var.width == width).collect();
                IntOperand::Var(same[rng.random_range(0..same.len())].name.clone())
            } else {
                IntOperand::Var(self.opaque_of(width, rng))
            }
        } else {
            if !left_opaque {
                left = self.opaque_of(width, rng);
            }
            let mut value = wild_value(width, rng);
            if matches!(op, IntOp::Div | IntOp::Rem) && value == 0 {
                value = 1;
            }
            IntOperand::Literal(value)
        };
        let name = self.fresh();
        self.statements.push(NumericStatement::LetBinary {
            name: name.clone(),
            width,
            op,
            left,
            right,
        });
        self.ints.push(IntVar {
            name,
            width,
            opaque: true,
        });
    }

    fn compound(&mut self, rng: &mut StdRng) {
        let picked = self.pick_int(rng);
        let width = self.ints[picked].width;
        let target_opaque = self.ints[picked].opaque;
        let target = self.ints[picked].name.clone();
        let op = random_op(rng);
        let operand = if !target_opaque || rng.random_bool(0.5) {
            IntOperand::Var(self.opaque_of(width, rng))
        } else {
            let mut value = wild_value(width, rng);
            if matches!(op, IntOp::Div | IntOp::Rem) && value == 0 {
                value = 1;
            }
            IntOperand::Literal(value)
        };
        self.statements.push(NumericStatement::Compound {
            target: target.clone(),
            op,
            operand,
        });
        if let Some(var) = self.ints.iter_mut().find(|var| var.name == target) {
            var.opaque = true;
        }
    }

    fn shift(&mut self, rng: &mut StdRng) {
        let picked = self.pick_int(rng);
        let width = self.ints[picked].width;
        let source_opaque = self.ints[picked].opaque;
        let source = self.ints[picked].name.clone();
        let direction = if rng.random_bool(0.5) {
            ShiftDirection::Left
        } else {
            ShiftDirection::Right
        };
        let oversized = rng.random_bool(0.4);
        let amount = if oversized {
            width.bits() + rng.random_range(0..=64)
        } else {
            rng.random_range(0..width.bits())
        };
        let name = self.fresh();
        self.statements.push(NumericStatement::Shift {
            name: name.clone(),
            width,
            source,
            direction,
            amount,
            oversized,
        });
        self.ints.push(IntVar {
            name,
            width,
            opaque: source_opaque || oversized,
        });
    }

    fn negate(&mut self, rng: &mut StdRng) {
        let signed = [IntWidth::I8, IntWidth::I16, IntWidth::I32, IntWidth::I64];
        let width = signed[rng.random_range(0..signed.len())];
        let source = self.opaque_of(width, rng);
        let name = self.fresh();
        self.statements.push(NumericStatement::Negate {
            name: name.clone(),
            width,
            source,
        });
        self.ints.push(IntVar {
            name,
            width,
            opaque: true,
        });
    }

    fn recast(&mut self, rng: &mut StdRng) {
        let picked = self.pick_int(rng);
        let source_opaque = self.ints[picked].opaque;
        let source = self.ints[picked].name.clone();
        let width = random_width(rng);
        let name = self.fresh();
        self.statements.push(NumericStatement::Recast {
            name: name.clone(),
            width,
            source,
        });
        self.ints.push(IntVar {
            name,
            width,
            opaque: source_opaque,
        });
    }

    fn float_let(&mut self, rng: &mut StdRng) -> usize {
        let width = random_float_width(rng);
        let name = self.fresh();
        self.statements.push(NumericStatement::FloatLet {
            name: name.clone(),
            width,
            token: wild_float(width, rng).to_string(),
        });
        self.floats.push(FloatVar { name, width });
        self.floats.len() - 1
    }

    fn pick_float(&mut self, rng: &mut StdRng) -> usize {
        if self.floats.is_empty() {
            return self.float_let(rng);
        }
        rng.random_range(0..self.floats.len())
    }

    fn float_binary(&mut self, rng: &mut StdRng) {
        let picked = self.pick_float(rng);
        let width = self.floats[picked].width;
        let left = self.floats[picked].name.clone();
        let op = match rng.random_range(0..4) {
            0 => FloatOp::Add,
            1 => FloatOp::Sub,
            2 => FloatOp::Mul,
            _ => FloatOp::Div,
        };
        let right = if rng.random_bool(0.5) {
            let same: Vec<&FloatVar> = self
                .floats
                .iter()
                .filter(|var| var.width == width)
                .collect();
            FloatOperand::Var(same[rng.random_range(0..same.len())].name.clone())
        } else {
            FloatOperand::Token(wild_float(width, rng).to_string())
        };
        let name = self.fresh();
        self.statements.push(NumericStatement::FloatBinary {
            name: name.clone(),
            width,
            op,
            left,
            right,
        });
        self.floats.push(FloatVar { name, width });
    }

    fn int_to_float(&mut self, rng: &mut StdRng) {
        let picked = self.pick_int(rng);
        let source = self.ints[picked].name.clone();
        let width = random_float_width(rng);
        let name = self.fresh();
        self.statements.push(NumericStatement::IntToFloat {
            name: name.clone(),
            width,
            source,
        });
        self.floats.push(FloatVar { name, width });
    }

    fn float_to_int(&mut self, rng: &mut StdRng) {
        let picked = self.pick_float(rng);
        let source = self.floats[picked].name.clone();
        let width = random_width(rng);
        let name = self.fresh();
        self.statements.push(NumericStatement::FloatToInt {
            name: name.clone(),
            width,
            source,
        });
        // A float to int cast saturates instead of panicking, so the result
        // is safe to fold even when the compiler can see the float.
        self.ints.push(IntVar {
            name,
            width,
            opaque: true,
        });
    }
}

/// Narrow widths carry double weight, they are where an interpreter with one
/// internal integer type diverges most.
fn random_width(rng: &mut StdRng) -> IntWidth {
    const WIDTHS: &[IntWidth] = &[
        IntWidth::U8,
        IntWidth::U8,
        IntWidth::U16,
        IntWidth::U16,
        IntWidth::U32,
        IntWidth::U32,
        IntWidth::I8,
        IntWidth::I8,
        IntWidth::I16,
        IntWidth::I16,
        IntWidth::I32,
        IntWidth::I32,
        IntWidth::U64,
        IntWidth::USize,
        IntWidth::I64,
    ];
    WIDTHS[rng.random_range(0..WIDTHS.len())]
}

fn random_float_width(rng: &mut StdRng) -> FloatWidth {
    if rng.random_bool(0.6) {
        FloatWidth::F32
    } else {
        FloatWidth::F64
    }
}

fn random_op(rng: &mut StdRng) -> IntOp {
    match rng.random_range(0..5) {
        0 => IntOp::Add,
        1 => IntOp::Sub,
        2 => IntOp::Mul,
        3 => IntOp::Div,
        _ => IntOp::Rem,
    }
}

/// Values on and next to the width's own boundaries, plus the boundaries of
/// every narrower width, so casts and arithmetic cross representation edges.
fn wild_value(width: IntWidth, rng: &mut StdRng) -> i128 {
    const CROSSERS: &[i128] = &[
        -129,
        -128,
        -127,
        127,
        128,
        255,
        256,
        -32_769,
        -32_768,
        32_767,
        32_768,
        65_535,
        65_536,
        -2_147_483_649,
        -2_147_483_648,
        2_147_483_647,
        2_147_483_648,
        4_294_967_295,
        4_294_967_296,
    ];
    let pick = rng.random_range(0..10);
    let value = match pick {
        0 => width.min(),
        1 => width.min().saturating_add(1),
        2 => width.max(),
        3 => width.max() - 1,
        4 => width.max() / 2,
        5..=6 => CROSSERS[rng.random_range(0..CROSSERS.len())],
        _ => i128::from(rng.random_range(-50i64..=50)),
    };
    value.clamp(width.min(), width.max())
}

/// Raw i64 values fed through `diff_opaque` before a cast to the width. Out
/// of range values are the point, the cast truncates them in compiled Rust.
fn wild_opaque(width: IntWidth, rng: &mut StdRng) -> i64 {
    let min = i64::try_from(width.min()).unwrap_or(i64::MIN);
    let max = i64::try_from(width.max()).unwrap_or(i64::MAX);
    match rng.random_range(0..10) {
        0 => min,
        1 => max,
        2 => max.saturating_add(1),
        3 => min.saturating_sub(1),
        4 => -1,
        5 => i64::MAX,
        6 => i64::MIN,
        7 => max.saturating_mul(2),
        _ => rng.random_range(-50..=50),
    }
}

/// Tokens whose exact printed form separates a real f32 from an f64 kept
/// internally: values not representable in f32, the shortest round trip
/// strings, extremes, subnormals, and the specials.
fn wild_float(width: FloatWidth, rng: &mut StdRng) -> &'static str {
    const F32: &[&str] = &[
        "0.0",
        "-0.0",
        "1.0",
        "0.5",
        "0.1",
        "0.2",
        "1.5",
        "2.5",
        "10.25",
        "0.3",
        "16777216.0",
        "16777217.0",
        "0.30000001",
        "3.4028235e38",
        "-3.4028235e38",
        "1e-45",
        "1.1754944e-38",
        "f32::NAN",
        "f32::INFINITY",
        "f32::NEG_INFINITY",
        "f32::EPSILON",
    ];
    const F64: &[&str] = &[
        "0.0",
        "-0.0",
        "1.0",
        "0.5",
        "0.1",
        "0.2",
        "1.5",
        "2.5",
        "10.25",
        "0.30000000000000004",
        "9007199254740993.0",
        "1e300",
        "-1e300",
        "5e-324",
        "1.7976931348623157e308",
        "f64::NAN",
        "f64::INFINITY",
        "f64::NEG_INFINITY",
        "f64::EPSILON",
    ];
    let pool = match width {
        FloatWidth::F32 => F32,
        FloatWidth::F64 => F64,
    };
    pool[rng.random_range(0..pool.len())]
}
