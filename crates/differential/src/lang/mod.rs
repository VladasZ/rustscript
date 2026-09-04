//! A type directed Rust program generator. Ask for a `u8` and the solver offers every shape that
//! can produce one, so a catalog row composes at any depth.

pub mod block;
pub mod catalog;
pub mod expr;
mod expr_render;
mod expr_walk;
pub mod fmt;
pub mod own;
mod own_check;
pub mod pat;
pub mod pipe;
pub mod stmt;
mod stmt_render;
mod stmt_walk;
pub mod synth;
pub mod ty;
pub mod user;
pub mod width;

use rand::rngs::StdRng;

pub use block::Block;
pub use expr::Expr;
pub use stmt::Stmt;
pub use ty::Ty;

/// Every item and binding name carries `tag`, so 2 blocks never collide.
pub fn generate_block(rng: &mut StdRng, tag: usize) -> Block {
    synth::Generator::new(rng, tag).block()
}
