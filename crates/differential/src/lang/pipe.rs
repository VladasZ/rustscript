//! Iterator pipelines, a source, adapters and a terminal.
//!
//! Determinism rule. A map or set source iterates in an order real Rust randomizes, so such a pipe
//! passes a `Sorted` stage before anything order sensitive, float items and fallible closures included.
//!
//! Panic reach rule. std collects a `Vec` into a `Vec` in place and touches no item when a `Skip`
//! emptied it, so a panicking body before a `Skip` never runs there while a lazy engine runs it. A
//! `Sorted` stage runs every body before it, so it clears the flag.
//!
//! Form rule. `rev` needs a double ended chain, `rposition` one that also knows its length, and
//! `flatten`, `unzip` and the like need the item type they work on, see `Caps` and `fits`.
//!
//! `is_valid` re-checks all 3 on every generated and shrunk pipe.

mod source;
mod stage;
mod term;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{Expr, Helper};
use crate::lang::ref_param::RefParam;
use crate::lang::ty::Ty;

pub use source::{Access, Caps, Item, Source};
pub use stage::{Bind, ParamAnn, Stage, held_names};
pub use term::{Site, Term};

/// A `Sorted` runs every body before it, so it clears the flag.
fn carries_panic(pending: bool, stage: &Stage, item: &Item) -> bool {
    match stage {
        Stage::Sorted => false,
        _ => pending || stage.observable(item),
    }
}

/// A `Skip` appended here would hide a body's run, see the panic reach rule.
pub fn fallible_pending(source: &Item, stages: &[Stage]) -> bool {
    let mut pending = false;
    let mut item = source.clone();
    for stage in stages {
        pending = carries_panic(pending, stage, &item);
        item = stage.out(&item);
    }
    pending
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Pipe {
    pub source: Source,
    pub stages: Vec<Stage>,
    pub term: Term,
}

impl Pipe {
    pub fn final_item(&self) -> Item {
        let mut item = self.source.item();
        for stage in &self.stages {
            item = stage.out(&item);
        }
        item
    }

    pub fn ty(&self) -> Ty {
        self.term.ty(&self.final_item())
    }

    /// A bare `collect`, `sum` or `product` needs the context to state it.
    pub fn states_type(&self) -> bool {
        !self.term.is_bare()
    }

    /// The 3 rules from the module docs. A pipe that fails one is a harness bug.
    pub fn is_valid(&self) -> bool {
        self.is_deterministic() && self.panics_reach_output() && self.is_well_formed()
    }

    /// The form rule.
    pub fn is_well_formed(&self) -> bool {
        let mut item = self.source.item();
        let mut caps = self.source.caps();
        for stage in &self.stages {
            if !stage.fits(&item, caps) {
                return false;
            }
            caps = stage.caps(caps);
            item = stage.out(&item);
        }
        self.term.fits(&item, caps)
    }

    /// The panic reach rule.
    pub fn panics_reach_output(&self) -> bool {
        let mut pending = false;
        let mut item = self.source.item();
        for stage in &self.stages {
            if matches!(stage, Stage::Skip(_)) && pending {
                return false;
            }
            pending = carries_panic(pending, stage, &item);
            item = stage.out(&item);
        }
        true
    }

    /// The determinism rule.
    pub fn is_deterministic(&self) -> bool {
        let mut ordered = self.source.ordered();
        let mut item = self.source.item();
        for stage in &self.stages {
            if matches!(stage, Stage::Sorted) {
                if !item.is_ord() {
                    return false;
                }
                ordered = true;
            }
            if !ordered && (stage.order_sensitive() || stage.leaks_order(&item)) {
                return false;
            }
            let computed = matches!(
                stage,
                Stage::Map { .. } | Stage::FilterMap { .. } | Stage::FlatMap { .. }
            );
            item = stage.out(&item);
            // computed items may be floats only if order is defined
            if computed && !ordered && item.is_float() {
                return false;
            }
        }
        if !ordered && (self.term.order_sensitive(&item) || self.term.leaks_order(&item)) {
            return false;
        }
        true
    }

    pub fn render(&self) -> String {
        let mut out = self.source.render();
        let mut item = self.source.item();
        for stage in &self.stages {
            let ty = item.rust();
            match stage {
                Stage::Sorted => {
                    out = format!(
                        "({{ let mut diff_sorted: Vec<{ty}> = {out}.collect(); diff_sorted.sort(); diff_sorted }}).into_iter()"
                    );
                }
                Stage::ByRefTake(count) => {
                    out = format!(
                        "({{ let mut diff_it = {out}; let diff_head: Vec<{ty}> = diff_it.by_ref().take({count}usize).collect(); diff_it.chain(diff_head) }})"
                    );
                }
                _ => out.push_str(&stage.render(&item)),
            }
            item = stage.out(&item);
        }
        format!("({})", self.term.render_onto(&out, &item))
    }

    /// The source and the closure bodies.
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        if let Source::Coll { expr, .. } = &self.source {
            out.push(expr);
        }
        for stage in &self.stages {
            out.extend(stage.exprs());
        }
        out.extend(self.term.exprs());
        out
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        if let Source::Coll { expr, .. } = &mut self.source {
            out.push(expr);
        }
        for stage in &mut self.stages {
            out.extend(stage.exprs_mut());
        }
        out.extend(self.term.exprs_mut());
        out
    }

    /// Every name a closure of the pipe binds.
    pub fn binds(&self) -> Vec<&Bind> {
        let mut out: Vec<&Bind> = self.stages.iter().flat_map(Stage::binds).collect();
        out.extend(self.term.binds());
        out
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        self.exprs().iter().any(|expr| expr.uses_any(names))
    }

    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        for expr in self.exprs() {
            expr.helpers(out);
        }
    }

    pub fn make_opaque(&mut self) {
        for expr in self.exprs_mut() {
            expr.make_opaque();
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert("lang-pipe");
        out.extend(self.source.feature());
        out.extend(self.term.features());
        out.extend(self.term.ref_param().and_then(RefParam::feature));
        for stage in &self.stages {
            out.insert(stage.feature());
            out.extend(stage.ref_param().and_then(RefParam::feature));
        }
        for expr in self.exprs() {
            expr.features(out);
        }
    }

    pub fn shrinks(&self) -> Vec<Pipe> {
        let mut out = Vec::new();
        for index in 0..self.stages.len() {
            if self.stages[index].removable() {
                let mut candidate = self.clone();
                candidate.stages.remove(index);
                if candidate.is_valid() {
                    out.push(candidate);
                }
            }
        }
        for (index, expr) in self.exprs().iter().enumerate() {
            for shrunk in expr.shrinks() {
                let mut candidate = self.clone();
                if let Some(slot) = candidate.exprs_mut().into_iter().nth(index) {
                    *slot = shrunk;
                }
                if candidate.is_valid() {
                    out.push(candidate);
                }
            }
        }
        out
    }
}
