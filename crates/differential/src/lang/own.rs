//! Ownership. A binding is owned, moved, or partially moved by field. The generator asks `Scope`
//! before every read and records what it chose, and `check_block` in `own_check` replays the
//! finished tree with the same rules, so a shrunk or spliced program that reads a moved binding
//! is refused before `rustc` sees it.
//!
//! The rules are a subset of what `rustc` accepts, never a superset. A move is offered only at
//! the loop and closure depth the binding was declared at, a `match` scrutinee or a method
//! receiver read by move counts as moved even where `rustc` would only borrow it, and a binding
//! revived inside a loop or one branch stays moved after it.

use std::collections::BTreeSet;

use crate::lang::expr::Expr;
use crate::lang::ty::Ty;

pub use crate::lang::own_check::check_block;

#[derive(Clone, Debug)]
pub enum BindKind {
    Local,
    Const,
    Closure { params: Vec<Ty>, ret: Ty },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnState {
    Owned,
    Moved,
    /// the fields moved out so far
    Partial(BTreeSet<usize>),
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub name: String,
    pub ty: Ty,
    pub kind: BindKind,
    pub state: OwnState,
    /// declared by a `let`, so it can be `mut` and written in place. A parameter or a pattern
    /// binding can only be read or moved.
    place: bool,
    /// behind a reference, `self.f0` in a `&self` method, so a read must clone
    borrowed: bool,
    loop_depth: usize,
    closure_depth: usize,
}

impl Slot {
    /// A closure binding carries its return type, but the closure itself never copies.
    pub fn is_copy(&self) -> bool {
        matches!(self.kind, BindKind::Local | BindKind::Const) && self.ty.is_copy()
    }
}

/// Bindings lifted out of the scope for a while, see `Scope::hide`.
pub struct Hidden {
    /// each index counted after the ones before it were removed, so they go back last first
    slots: Vec<(usize, Slot)>,
}

pub type Snapshot = Vec<(String, OwnState)>;

#[derive(Default)]
pub struct Scope {
    slots: Vec<Slot>,
    /// names a statement holds while its parts run, a move or an in place write of one would
    /// be a second borrow
    frozen: Vec<String>,
    /// names a construct reads through a shared reference for its whole body, `for x in &v`.
    /// A second shared borrow is fine, a move or a write is not.
    shared: Vec<String>,
    /// names borrowed by a reference that lives to the end of the statement, see `stmt_mark`
    stmt_shared: Vec<String>,
    /// names borrowed by a reference a `let` keeps, with the scope depth the `let` sits at.
    /// The borrow ends when that scope does.
    pinned: Vec<(String, usize)>,
    scope_depth: usize,
    loop_depth: usize,
    closure_depth: usize,
    /// contexts that only borrow, a print or a comparison, so a move there is wasted
    no_move: usize,
}

impl Scope {
    /// A parameter, a pattern binding or a closure, readable and movable but never written.
    pub fn push(&mut self, name: String, ty: Ty, kind: BindKind) {
        self.push_slot(name, ty, kind, false, false);
    }

    /// A `let` binding, which a later write may make `mut`.
    pub fn push_let(&mut self, name: String, ty: Ty) {
        self.push_slot(name, ty, BindKind::Local, true, false);
    }

    /// A place behind a reference, see `Slot::borrowed`.
    pub fn push_borrowed(&mut self, name: String, ty: Ty) {
        self.push_slot(name, ty, BindKind::Local, false, true);
    }

    fn push_slot(&mut self, name: String, ty: Ty, kind: BindKind, place: bool, borrowed: bool) {
        self.slots.push(Slot {
            name,
            ty,
            kind,
            state: OwnState::Owned,
            place,
            borrowed,
            loop_depth: self.loop_depth,
            closure_depth: self.closure_depth,
        });
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Opens a block scope. The mark goes back to `exit_scope`.
    pub fn enter_scope(&mut self) -> usize {
        self.scope_depth += 1;
        self.slots.len()
    }

    /// The bindings declared since the mark are gone, and so is every borrow a `let` of this
    /// scope kept.
    pub fn exit_scope(&mut self, mark: usize) {
        self.slots.truncate(mark);
        let depth = self.scope_depth;
        self.pinned.retain(|(_, pinned_at)| *pinned_at < depth);
        self.scope_depth -= 1;
    }

    pub fn take_slots(&mut self) -> Vec<Slot> {
        std::mem::take(&mut self.slots)
    }

    pub fn set_slots(&mut self, slots: Vec<Slot>) {
        self.slots = slots;
    }

    /// The latest binding of the name, the one a read resolves to.
    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().rev().find(|slot| slot.name == name)
    }

    fn slot_mut(&mut self, name: &str) -> Option<&mut Slot> {
        self.slots.iter_mut().rev().find(|slot| slot.name == name)
    }

    /// Every binding a name resolves to, in declaration order.
    pub fn visible(&self) -> Vec<&Slot> {
        let mut seen = BTreeSet::new();
        let mut out: Vec<&Slot> = self
            .slots
            .iter()
            .rev()
            .filter(|slot| seen.insert(slot.name.as_str()))
            .collect();
        out.reverse();
        out
    }

    /// Every binding of the name goes, a shadowed one under it would answer to the name too.
    pub fn hide(&mut self, name: &str) -> Option<Hidden> {
        let mut slots = Vec::new();
        let mut index = 0;
        while index < self.slots.len() {
            if self.slots[index].name == name {
                slots.push((index, self.slots.remove(index)));
            } else {
                index += 1;
            }
        }
        (!slots.is_empty()).then_some(Hidden { slots })
    }

    pub fn unhide(&mut self, hidden: Hidden) {
        for (index, slot) in hidden.slots.into_iter().rev() {
            let index = index.min(self.slots.len());
            self.slots.insert(index, slot);
        }
    }

    pub fn freeze(&mut self, name: &str) {
        self.frozen.push(name.to_string());
    }

    pub fn unfreeze(&mut self) {
        self.frozen.pop();
    }

    pub fn hold_shared(&mut self, name: &str) {
        self.shared.push(name.to_string());
    }

    pub fn release_shared(&mut self) {
        self.shared.pop();
    }

    /// A reference to the binding lives until the statement ends.
    pub fn borrow_for_stmt(&mut self, name: &str) {
        self.stmt_shared.push(name.to_string());
    }

    /// Taken when a statement starts, `stmt_release` ends the borrows made since.
    pub fn stmt_mark(&self) -> usize {
        self.stmt_shared.len()
    }

    pub fn stmt_release(&mut self, mark: usize) {
        self.stmt_shared.truncate(mark);
    }

    /// A `let` keeps a reference to the binding, so it is borrowed until the scope ends.
    pub fn pin(&mut self, name: &str) {
        self.pinned.push((name.to_string(), self.scope_depth));
    }

    fn is_held(&self, name: &str) -> bool {
        self.frozen.iter().any(|frozen| frozen == name)
    }

    fn is_frozen(&self, name: &str) -> bool {
        self.is_held(name)
            || self.shared.iter().any(|held| held == name)
            || self.stmt_shared.iter().any(|held| held == name)
            || self.pinned.iter().any(|(held, _)| held == name)
    }

    pub fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    pub fn leave_loop(&mut self) {
        self.loop_depth -= 1;
    }

    pub fn enter_closure(&mut self) {
        self.closure_depth += 1;
    }

    pub fn leave_closure(&mut self) {
        self.closure_depth -= 1;
    }

    pub fn forbid_moves(&mut self) {
        self.no_move += 1;
    }

    pub fn allow_moves(&mut self) {
        self.no_move -= 1;
    }

    fn at_own_depth(&self, slot: &Slot) -> bool {
        slot.loop_depth == self.loop_depth && slot.closure_depth == self.closure_depth
    }

    pub fn can_read(&self, name: &str) -> bool {
        self.slot(name)
            .is_some_and(|slot| slot.state == OwnState::Owned)
    }

    pub fn can_read_field(&self, name: &str, index: usize) -> bool {
        self.slot(name).is_some_and(|slot| match &slot.state {
            OwnState::Owned => true,
            OwnState::Moved => false,
            OwnState::Partial(gone) => !gone.contains(&index),
        })
    }

    pub fn can_move(&self, name: &str) -> bool {
        self.no_move == 0
            && !self.is_frozen(name)
            && self.slot(name).is_some_and(|slot| {
                !slot.is_copy()
                    && !slot.borrowed
                    && slot.state == OwnState::Owned
                    && self.at_own_depth(slot)
            })
    }

    pub fn can_move_field(&self, name: &str, index: usize, field: &Ty) -> bool {
        self.no_move == 0
            && !field.is_copy()
            && !self.is_frozen(name)
            && self.can_read_field(name, index)
            && self
                .slot(name)
                .is_some_and(|slot| !slot.borrowed && self.at_own_depth(slot))
    }

    /// `std::mem` writes and vec take outs borrow the binding mutably for the call alone.
    pub fn can_mem(&self, name: &str) -> bool {
        !self.is_frozen(name)
            && self.slot(name).is_some_and(|slot| {
                slot.place
                    && slot.state == OwnState::Owned
                    && slot.closure_depth == self.closure_depth
            })
    }

    /// A write in place, `push`, `+=`, `&mut name`. The binding must be a `let` so it can be
    /// `mut`, and nothing else may hold it.
    pub fn can_write(&self, name: &str) -> bool {
        self.can_compound(name)
            && self
                .slot(name)
                .is_some_and(|slot| slot.closure_depth == self.closure_depth)
    }

    /// `name op= value`. A closure body may write a captured counter, so the closure depth
    /// is not checked.
    pub fn can_compound(&self, name: &str) -> bool {
        !self.is_frozen(name)
            && self
                .slot(name)
                .is_some_and(|slot| slot.place && slot.state == OwnState::Owned)
    }

    /// A shared reference to the binding, `name.as_str()`. A pattern binding or a parameter
    /// may die before the reference does, so only a `let` is offered.
    pub fn can_borrow(&self, name: &str) -> bool {
        !self.is_held(name)
            && self.slot(name).is_some_and(|slot| {
                slot.place
                    && !slot.borrowed
                    && slot.state == OwnState::Owned
                    && slot.closure_depth == self.closure_depth
            })
    }

    /// A moved binding is assigned back to life.
    pub fn can_assign(&self, name: &str) -> bool {
        !self.is_frozen(name)
            && self
                .slot(name)
                .is_some_and(|slot| slot.place && slot.closure_depth == self.closure_depth)
    }

    pub fn can_assign_field(&self, name: &str, index: usize) -> bool {
        !self.is_frozen(name)
            && self.slot(name).is_some_and(|slot| {
                slot.place
                    && slot.closure_depth == self.closure_depth
                    && match &slot.state {
                        OwnState::Owned | OwnState::Partial(_) => true,
                        OwnState::Moved => false,
                    }
                    && index < field_count(&slot.ty)
            })
    }

    pub fn note_move(&mut self, name: &str) {
        if let Some(slot) = self.slot_mut(name) {
            slot.state = OwnState::Moved;
        }
    }

    pub fn note_field_move(&mut self, name: &str, index: usize) {
        if let Some(slot) = self.slot_mut(name) {
            let mut gone = match &slot.state {
                OwnState::Partial(gone) => gone.clone(),
                _ => BTreeSet::new(),
            };
            gone.insert(index);
            slot.state = OwnState::Partial(gone);
        }
    }

    pub fn revive(&mut self, name: &str) {
        if let Some(slot) = self.slot_mut(name) {
            slot.state = OwnState::Owned;
        }
    }

    pub fn revive_field(&mut self, name: &str, index: usize) {
        if let Some(slot) = self.slot_mut(name)
            && let OwnState::Partial(gone) = &mut slot.state
        {
            gone.remove(&index);
            if gone.is_empty() {
                slot.state = OwnState::Owned;
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.visible()
            .into_iter()
            .map(|slot| (slot.name.clone(), slot.state.clone()))
            .collect()
    }

    pub fn restore(&mut self, snapshot: &Snapshot) {
        for (name, state) in snapshot {
            if let Some(slot) = self.slot_mut(name) {
                slot.state = state.clone();
            }
        }
    }

    /// After branches. A binding moved on any path is moved, a field moved on any path is gone.
    pub fn merge(&mut self, before: &Snapshot, ends: &[Snapshot]) {
        for (name, _) in before {
            let mut gone = BTreeSet::new();
            let mut moved = false;
            for end in ends {
                match end.iter().find(|(seen, _)| seen == name).map(|(_, s)| s) {
                    Some(OwnState::Moved) | None => moved = true,
                    Some(OwnState::Partial(fields)) => gone.extend(fields.iter().copied()),
                    Some(OwnState::Owned) => {}
                }
            }
            let state = if moved {
                OwnState::Moved
            } else if gone.is_empty() {
                OwnState::Owned
            } else {
                OwnState::Partial(gone)
            };
            if let Some(slot) = self.slot_mut(name) {
                slot.state = state;
            }
        }
    }
}

fn field_count(ty: &Ty) -> usize {
    match ty {
        Ty::User(shape) => shape.fields().len(),
        Ty::Tuple(items) => items.len(),
        _ => 0,
    }
}

/// The bindings an expression names anywhere inside it.
pub fn referenced(expr: &Expr) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for node in expr.nodes() {
        match node {
            Expr::Var { name, .. }
            | Expr::Mem { name, .. }
            | Expr::VecTake { name, .. }
            | Expr::ClosureCall { name, .. } => {
                out.insert(name.clone());
            }
            Expr::ApplyCall { closure, .. } => {
                out.insert(closure.clone());
            }
            _ => {}
        }
    }
    out
}

/// The binding a place expression reads, through fields and indexes. A receiver rooted in
/// one is borrowed while the arguments run, see `Method::borrows_recv`.
pub fn root_binding(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var { name, .. } => Some(name),
        Expr::Field { base, .. }
        | Expr::TupleField { base, .. }
        | Expr::Index { base, .. }
        | Expr::Borrow { base, .. } => root_binding(base),
        _ => None,
    }
}
