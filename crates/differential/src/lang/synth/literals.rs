//! Literal generation, one value of every scalar type.

use rand::RngExt;

use crate::lang::expr::Expr;
use crate::lang::synth::Generator;
use crate::lang::ty::{FloatWidth, IntWidth, Ty};

impl Generator<'_> {
    pub(super) fn literal(&mut self, want: &Ty) -> Expr {
        match want {
            Ty::Int(width) => Expr::IntLit {
                width: *width,
                value: self.int_value(*width),
                opaque: false,
            },
            Ty::Float(width) => Expr::FloatLit {
                width: *width,
                token: self.float_token(*width),
                opaque: false,
            },
            Ty::Bool => Expr::BoolLit {
                value: self.chance(0.5),
                opaque: false,
            },
            Ty::Char => Expr::CharLit {
                value: self.char_value(),
                opaque: false,
            },
            Ty::Str => Expr::StrLit(self.string_value()),
            Ty::Vec(elem) => {
                let count = self.rng.random_range(0..=3);
                // a repeat of a nested container is where shared rows would show
                if self.chance(0.3) {
                    return Expr::VecRepeat {
                        elem: (**elem).clone(),
                        item: Box::new(self.leaf(elem)),
                        count,
                    };
                }
                let items = (0..count).map(|_| self.leaf(elem)).collect();
                Expr::VecLit {
                    elem: (**elem).clone(),
                    items,
                }
            }
            Ty::Opt(elem) => {
                let value = if self.chance(0.65) {
                    Some(Box::new(self.leaf(elem)))
                } else {
                    None
                };
                Expr::OptLit {
                    elem: (**elem).clone(),
                    value,
                }
            }
            Ty::Map(key, value) => {
                let count = self.rng.random_range(0..=3);
                let items = (0..count)
                    .map(|_| (self.leaf(key), self.leaf(value)))
                    .collect();
                Expr::MapLit {
                    key: (**key).clone(),
                    value: (**value).clone(),
                    items,
                }
            }
            Ty::Set(elem) => {
                let count = self.rng.random_range(0..=3);
                let items = (0..count).map(|_| self.leaf(elem)).collect();
                Expr::SetLit {
                    elem: (**elem).clone(),
                    items,
                }
            }
            Ty::Tuple(items) => Expr::TupleLit(items.iter().map(|item| self.leaf(item)).collect()),
            Ty::Res(ok, err) => {
                let value = if self.chance(0.6) {
                    Ok(Box::new(self.leaf(ok)))
                } else {
                    Err(Box::new(self.leaf(err)))
                };
                Expr::ResLit {
                    ok: (**ok).clone(),
                    err: (**err).clone(),
                    value,
                }
            }
            Ty::StdErr(err) => Expr::StdErrLit(*err),
            Ty::User(shape) => self.user_literal(shape, 0),
        }
    }

    /// Boundary values first, a width bug shows at the edge of the range.
    pub(super) fn int_value(&mut self, width: IntWidth) -> i128 {
        let (min, max) = (width.min(), width.max());
        match self.rng.random_range(0..12) {
            0 => 0,
            1 => 1,
            2 => max,
            3 => max - 1,
            4 => min,
            5 if min != 0 => min + 1,
            6 => max / 2,
            7 => max / 2 + 1,
            8 if min != 0 => -1,
            9 => 2,
            _ => {
                let span = max - min;
                let draw = i128::from(self.rng.random_range(0..=u64::MAX));
                min + draw.rem_euclid(span + 1)
            }
        }
    }

    pub(super) fn float_token(&mut self, width: FloatWidth) -> String {
        let suffix = width.rust();
        match self.rng.random_range(0..12) {
            0 => format!("{suffix}::NAN"),
            1 => format!("{suffix}::INFINITY"),
            2 => format!("{suffix}::NEG_INFINITY"),
            3 => format!("{suffix}::MAX"),
            4 => format!("{suffix}::MIN"),
            5 => format!("{suffix}::EPSILON"),
            6 => format!("0.0{suffix}"),
            7 => format!("(-0.0{suffix})"),
            8 => format!("1.0{suffix}"),
            9 => format!("(-1.0{suffix})"),
            10 => format!("0.5{suffix}"),
            _ => {
                let value = f64::from(self.rng.random_range(0..2_000_000)) / 1000.0 - 1000.0;
                // a bare negative literal binds looser than a method call
                if value < 0.0 {
                    format!("({value:?}{suffix})")
                } else {
                    format!("{value:?}{suffix}")
                }
            }
        }
    }

    pub(super) fn bare_float_token(&mut self) -> String {
        match self.rng.random_range(0..6) {
            0 => "0.0".to_string(),
            1 => "1.5".to_string(),
            2 => "(-2.25)".to_string(),
            3 => "1e10".to_string(),
            4 => "0.1".to_string(),
            _ => {
                let value = f64::from(self.rng.random_range(0..200_000)) / 100.0 - 1000.0;
                if value < 0.0 {
                    format!("({value:?})")
                } else {
                    format!("{value:?}")
                }
            }
        }
    }

    pub(super) fn char_value(&mut self) -> char {
        const POOL: &[char] = &[
            'a', 'Z', '0', '9', ' ', '\n', '\t', '_', 'é', 'ß', 'は', '✓', '7', 'f',
        ];
        *self.pick(POOL)
    }

    /// Full of things that look parseable on purpose, a pool of plain words would never exercise
    /// `parse`.
    pub(super) fn string_value(&mut self) -> String {
        const POOL: &[&str] = &[
            "",
            "0",
            "5",
            "-1",
            "300",
            "1.5",
            " 5 ",
            "5 ",
            "+7",
            "007",
            "true",
            "false",
            "TRUE",
            "1",
            "abc",
            "Hello World",
            "  padded  ",
            "a,b,c",
            "99999999999999999999",
            "-99999999999999999999",
            "0x1f",
            "1e3",
            "inf",
            "NaN",
            "é",
            "  ",
            "\n",
            "a\nb\nc",
            "key=value",
            "x,,y",
        ];
        (*self.pick(POOL)).to_string()
    }
}
