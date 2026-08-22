//! Structured mutation by subtree splicing. A same typed subtree from a
//! donor replaces a node, with its free variables fixed up for the target
//! scope. This creates nesting the top down generator never emits.

use std::collections::BTreeSet;

use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::generator::generate_base;
use crate::lang::expr::{Expr, minimal};
use crate::lang::pat::Pat;
use crate::lang::pipe::{Bind, Stage, Term};
use crate::lang::stmt::{Ann, Stmt};
use crate::lang::ty::Ty;
use crate::model::{MutationOperation, MutationOrigin, Program};

const MUTATION_SALT: u64 = 0x9E37_79B9_7F4A_7C15;

pub fn generate_or_mutate(seed: u64) -> Program {
    if seed != 0 && seed.is_multiple_of(4) {
        let parent_seed = seed.wrapping_sub(1);
        let parent = generate_base(parent_seed);
        mutate(&parent, parent_seed, seed, seed)
    } else {
        generate_base(seed)
    }
}

pub fn mutate(parent: &Program, parent_seed: u64, donor_seed: u64, output_seed: u64) -> Program {
    let donor = generate_base(donor_seed);
    let mut rng = StdRng::seed_from_u64(output_seed ^ MUTATION_SALT);
    let operation_count = rng.random_range(2..=4);
    let mut program = parent.clone();
    let mut operations = Vec::with_capacity(operation_count);
    for _ in 0..operation_count {
        if rng.random_bool(0.85) && splice(&mut program, &donor, &mut rng) {
            operations.push(MutationOperation::Splice);
        } else {
            program.blocks.reverse();
            operations.push(MutationOperation::BlockOrder);
        }
    }
    // A spliced subtree can land in a receiver position, so the whole program
    // is repaired again after the operations.
    for block in &mut program.blocks {
        block.fix_apply_borrows();
    }
    program.seed = output_seed;
    program.mutation = Some(MutationOrigin {
        parent_seed,
        donor_seed,
        operations,
    });
    program
}

/// Whether a subtree can move between programs. It must name nothing only its
/// own program declares and need no function body or `let` annotation around
/// it.
fn is_portable(expr: &Expr) -> bool {
    expr.nodes().iter().all(|node| {
        let own = !matches!(
            node,
            Expr::Pipe(pipe) if pipe.term.is_bare()
        ) && !matches!(
            node,
            Expr::FnCall { .. }
                | Expr::ClosureCall { .. }
                | Expr::ApplyCall { .. }
                | Expr::ConstRef { .. }
                | Expr::Method { .. }
                | Expr::TraitCall { .. }
                | Expr::Try { .. }
                | Expr::Into { .. }
                | Expr::StructLit { .. }
                | Expr::EnumLit { .. }
                | Expr::Field { .. }
                | Expr::Block { .. }
        );
        let pats = match node {
            Expr::Match { arms, .. } => arms
                .iter()
                .all(|arm| !matches!(arm.pat, Pat::Variant { .. } | Pat::Struct { .. })),
            _ => true,
        };
        own && pats && !mentions_user(&node.ty())
    })
}

fn mentions_user(ty: &Ty) -> bool {
    match ty {
        Ty::User(_) => true,
        Ty::Vec(inner) | Ty::Opt(inner) | Ty::Set(inner) => mentions_user(inner),
        Ty::Map(key, value) | Ty::Res(key, value) => mentions_user(key) || mentions_user(value),
        Ty::Tuple(items) => items.iter().any(mentions_user),
        _ => false,
    }
}

/// Names bound inside the subtree itself, they travel with the graft.
fn binders(expr: &Expr, out: &mut BTreeSet<String>) {
    for node in expr.nodes() {
        match node {
            Expr::Pipe(pipe) => {
                for stage in &pipe.stages {
                    match stage {
                        Stage::Map { bind, .. }
                        | Stage::PairWith { bind, .. }
                        | Stage::Filter { bind, .. } => bind_names(bind, out),
                        _ => {}
                    }
                }
                match &pipe.term {
                    Term::Any { bind, .. }
                    | Term::All { bind, .. }
                    | Term::Position { bind, .. } => {
                        bind_names(bind, out);
                    }
                    Term::Fold { acc, bind, .. } => {
                        out.insert(acc.clone());
                        bind_names(bind, out);
                    }
                    _ => {}
                }
            }
            Expr::Match { arms, .. } => {
                for arm in arms {
                    let mut binds = Vec::new();
                    arm.pat.bindings(&mut binds);
                    out.extend(binds.into_iter().map(|(name, _)| name));
                }
            }
            _ => {}
        }
    }
}

fn bind_names(bind: &Bind, out: &mut BTreeSet<String>) {
    match bind {
        Bind::One(name) => {
            out.insert(name.clone());
        }
        Bind::Pair(key, value) => {
            out.insert(key.clone());
            out.insert(value.clone());
        }
    }
}

/// A free variable with no same typed binding in scope becomes the minimal
/// literal of its type.
fn rebind(
    expr: &mut Expr,
    environment: &[(String, Ty)],
    bound: &BTreeSet<String>,
    rng: &mut StdRng,
) {
    if let Expr::Var { name, ty } = expr {
        if bound.contains(name) {
            return;
        }
        let candidates: Vec<&(String, Ty)> = environment
            .iter()
            .filter(|(_, env_ty)| env_ty == ty)
            .collect();
        if candidates.is_empty() {
            *expr = minimal(ty);
        } else {
            name.clone_from(&candidates[rng.random_range(0..candidates.len())].0);
        }
        return;
    }
    for child in expr.children_mut() {
        rebind(child, environment, bound, rng);
    }
}

/// The statements a splice may target, each with the `let`s before it.
fn splice(program: &mut Program, donor: &Program, rng: &mut StdRng) -> bool {
    if program.blocks.is_empty() {
        return false;
    }
    let block_index = rng.random_range(0..program.blocks.len());
    let block = &mut program.blocks[block_index];
    let targets: Vec<usize> = block
        .statements
        .iter()
        .enumerate()
        .filter(|(_, stmt)| {
            matches!(
                stmt,
                Stmt::Let { .. } | Stmt::Assign { .. } | Stmt::Print { .. } | Stmt::Compound { .. }
            )
        })
        .map(|(index, _)| index)
        .collect();
    if targets.is_empty() {
        return false;
    }
    let stmt_index = targets[rng.random_range(0..targets.len())];
    let environment: Vec<(String, Ty)> = block.statements[..stmt_index]
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Let { name, ty, .. } => Some((name.clone(), ty.clone())),
            _ => None,
        })
        .collect();
    let donor_nodes: Vec<&Expr> = donor
        .blocks
        .iter()
        .flat_map(|block| block.statements.iter())
        .flat_map(Stmt::exprs)
        .flat_map(Expr::nodes)
        .filter(|node| is_portable(node))
        .collect();
    if donor_nodes.is_empty() {
        return false;
    }
    let stmt = &mut block.statements[stmt_index];
    let mut slots = stmt.exprs_mut();
    if slots.is_empty() {
        return false;
    }
    let slot_index = rng.random_range(0..slots.len());
    let slot = &mut *slots[slot_index];
    let node_count = slot.nodes().len();
    let node_index = rng.random_range(0..node_count);
    let Some(target) = slot.nth_node_mut(node_index) else {
        return false;
    };
    let wanted = target.ty();
    let typed: Vec<&Expr> = donor_nodes
        .iter()
        .copied()
        .filter(|node| node.ty() == wanted)
        .collect();
    if typed.is_empty() {
        return false;
    }
    let mut graft = typed[rng.random_range(0..typed.len())].clone();
    let mut bound = BTreeSet::new();
    binders(&graft, &mut bound);
    rebind(&mut graft, &environment, &bound, rng);
    *target = graft;
    // A graft whose terminal states no type cannot initialize an unannotated
    // binding.
    if let Stmt::Let { expr, ann, .. } = stmt
        && let Expr::Pipe(pipe) = expr
        && !pipe.states_type()
    {
        *ann = Ann::Typed;
    }
    block.seal();
    true
}
