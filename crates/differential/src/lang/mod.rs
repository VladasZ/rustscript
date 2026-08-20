//! A type directed Rust program generator.
//!
//! The type universe is the real one, and generation is driven by types: ask
//! for a `u8` and the solver offers every literal, operator, cast, branch,
//! match, field, user method, conversion, and catalog method that can produce
//! one. A method is a single table row and it composes everywhere
//! immediately, at any depth. User types, consts, closures, and helper
//! functions are declared per block, so a `?` through a `From` impl can sit
//! inside a closure inside a fold over a `Vec<u8>` field of a struct.

pub mod block;
pub mod catalog;
pub mod expr;
mod expr_walk;
pub mod fmt;
pub mod pat;
pub mod pipe;
pub mod stmt;
pub mod synth;
pub mod ty;
pub mod user;
pub mod width;

use rand::rngs::StdRng;

pub use block::Block;
pub use expr::Expr;
pub use stmt::Stmt;
pub use ty::Ty;

/// `tag` distinguishes the blocks of one program. Every top level item and
/// binding name carries it, so two blocks never collide.
pub fn generate_block(rng: &mut StdRng, tag: usize) -> Block {
    synth::Generator::new(rng, tag).block()
}
