//! A type directed Rust program generator.
//!
//! The older generators are catalogs: a hand written enum variant per case,
//! each with its own render arm, printed as a standalone labeled line. Adding
//! coverage there means adding another variant, so the space explored grows
//! only as fast as someone writes rows, and whole dimensions stay invisible.
//! The `saturating_add` bug survived every campaign that way. The expression
//! grammar could name only `i64`, `bool` and `String`, so it generated the
//! method the whole time and never on the `u8` where the bug lived.
//!
//! Here the type universe is the real one, and generation is driven by types:
//! ask for a `u8` and the solver offers every literal, operator, cast, branch
//! and catalog method that can produce one. A method is a single table row and
//! it composes everywhere immediately, at any depth.

pub mod catalog;
pub mod expr;
pub mod stmt;
pub mod synth;
pub mod ty;

use rand::rngs::StdRng;

pub use expr::Expr;
pub use stmt::{Block, Stmt};
pub use ty::Ty;

pub fn generate_block(rng: &mut StdRng) -> Block {
    synth::Generator::new(rng).block()
}
