//! The per function compile state. Registers, scopes, upvalues and the finished `Chunk`.

use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;

use anyhow::Result;

use crate::interpreter::bytecode::{
    CapSource, Chunk, Const, DefaultIr, EnumVariant, FmtSpec, Member, MethodName, Op, PatInfo,
    PathRef, Reg, StructLit,
};
use crate::interpreter::typeir::{CastIr, TypeIr};

use super::idx16;

/// `moved[i]` is the new index of the op that was at `i`, one entry past the end included.
pub(super) fn retarget_jumps(code: &mut [Op], moved: &[u32]) {
    for op in code {
        match op {
            Op::Jump { to: t }
            | Op::JumpIfFalse { to: t, .. }
            | Op::JumpIfTrue { to: t, .. }
            | Op::CmpJump { to: t, .. }
            | Op::CmpJumpImm { to: t, .. }
            | Op::CmpJumpInt { to: t, .. }
            | Op::CmpJumpIntImm { to: t, .. }
            | Op::ForNext { to: t, .. }
            | Op::TryJump { to: t, .. } => *t = moved[*t as usize],
            _ => {}
        }
    }
}

/// Where a name was bound, so the binding can get its capture cell op.
#[derive(Clone, Copy)]
pub(super) struct BindingSite {
    /// the op index right after the binding
    pub(super) at: usize,
    pub(super) reg: Reg,
    /// the capture scan knew the binding needs a cell, so every access compiled as a cell access
    pub(super) cell: bool,
}

/// A stack of these supports nested closures.
pub(super) struct FnState {
    pub(super) code: Vec<Op>,
    pub(super) lines: Vec<u32>,
    pub(super) cols: Vec<u32>,
    pub(super) consts: Vec<Const>,
    pub(super) members: Vec<Member>,
    pub(super) pats: Vec<PatInfo>,
    pub(super) fmts: Vec<FmtSpec>,
    pub(super) struct_lits: Vec<StructLit>,
    pub(super) enum_variants: Vec<EnumVariant>,
    pub(super) casts: Vec<CastIr>,
    pub(super) defaults: Vec<DefaultIr>,
    pub(super) try_targets: Vec<Arc<str>>,
    /// the target a `?` converts into through `From`
    pub(super) ret_error: Option<Arc<str>>,
    pub(super) coerces: Vec<TypeIr>,
    pub(super) paths: Vec<PathRef>,
    pub(super) names: Vec<MethodName>,
    pub(super) children: Vec<Arc<Chunk>>,
    pub(super) child_caps: Vec<Vec<CapSource>>,
    /// Per capture of each child, whether the body reads only fields that copy, see
    /// `captures::captures_only_copy_fields`. A `move` closure then never takes the value.
    pub(super) child_partial: Vec<Vec<bool>>,
    /// filled by the liveness pass, see `liveness.rs`
    pub(super) child_moves: Vec<Arc<[bool]>>,
    pub(super) upvalues: Vec<(String, CapSource)>,
    pub(super) mutable_locals: HashSet<Reg>,
    /// The names the closures of the body write, see `captures`. Their bindings are cells.
    pub(super) cell_names: HashSet<String>,
    /// Every binding, so `into_chunk` can give the cell promoted ones their cell op once the
    /// frame is compiled.
    pub(super) binding_sites: Vec<BindingSite>,
    /// Reference parameters. They forward the caller's handle, so they are never moved, copied
    /// or dropped here.
    pub(super) borrow_params: HashSet<Reg>,
    /// `let r = &place` bindings, shared like a borrow parameter
    pub(super) ref_locals: HashSet<Reg>,
    /// Bindings that hold a borrowed handle, so scope end must not drop them.
    pub(super) drop_exempt: HashSet<Reg>,
    /// Bindings that hold an iterator over items of its own, so what `next` hands out drops.
    pub(super) owning_iters: HashSet<Reg>,
    /// `let r = &mut v` aliases, access compiles as access to `v` itself
    pub(super) aliases: HashMap<String, String>,
    /// Every change to `aliases` with the entry it replaced, so a scope end puts back what
    /// its aliases shadowed, see `Compiler::set_alias`.
    pub(super) alias_log: Vec<(String, Option<String>)>,
    /// The length of `alias_log` when each open scope began.
    pub(super) alias_marks: Vec<usize>,
    /// `const` and `static` items declared in a block. They are locals like a `let`, but a
    /// pattern that names one tests against its value.
    pub(super) block_consts: HashSet<String>,
    pub(super) scopes: Vec<HashMap<String, Reg>>,
    /// for scope end `Drop` runs
    pub(super) scope_order: Vec<Vec<Reg>>,
    pub(super) drop_lists: Vec<std::sync::Arc<[Reg]>>,
    /// see `Chunk::lent_params`
    pub(super) lent_params: Vec<Reg>,
    /// see `Chunk::lent_writebacks`, the op index is remapped with every insertion
    pub(super) lent_writebacks: Vec<(u32, u16, Reg)>,
    /// `borrow` results not yet released, see `release_guard_temps`
    pub(super) guard_temps: Vec<Reg>,
    /// Temporaries that own a fresh value, dropped at the end of their statement, see
    /// `drop_temps`. Only kept when the program has a `Drop` impl.
    pub(super) owned_temps: Vec<Reg>,
    /// Owned call arguments, taken by the call and so only dropped by a panic before it, see
    /// `compile_args`. Only kept when the program has a `Drop` impl. Each comes with the
    /// number of drop lists made when its op ran, `usize::MAX` while the op is still ahead,
    /// see `hold_operand`.
    pub(super) unwind_temps: Vec<(Reg, usize)>,
    /// The next call compiled is the tail expression of a block. Its owned operands then
    /// unwind after the temporaries its arguments made, not before, see `compile_args`.
    pub(super) tail_call: bool,
    /// named bindings that hold a `RefCell` guard, released at scope end even without `Drop` impls
    pub(super) guard_regs: HashSet<Reg>,
    pub(super) has_guards: bool,
    pub(super) reg_top: Reg,
    pub(super) max_reg: Reg,
    pub(super) num_params: usize,
    pub(super) param_types: Vec<Option<String>>,
    pub(super) name: String,
    pub(super) generics: Vec<Arc<str>>,
    pub(super) call_type_args: Vec<Arc<[TypeIr]>>,
    /// Retagging on the way out keeps the declared width without a cast at every call site.
    pub(super) ret_cast: Option<u16>,
}

impl FnState {
    /// Holds an owned operand for the unwinder until `close_operands` says its op runs.
    pub(super) fn hold_operand(&mut self, reg: Reg) {
        self.unwind_temps.push((reg, usize::MAX));
    }

    /// The op that takes the operands held since `from` comes next. The drop lists made from
    /// here on belong to the code around the op.
    pub(super) fn close_operands(&mut self, from: usize) {
        let until = self.drop_lists.len();
        for (_, open) in self.unwind_temps.iter_mut().skip(from) {
            if *open == usize::MAX {
                *open = until;
            }
        }
    }

    pub(super) fn new(name: String) -> FnState {
        FnState {
            code: Vec::new(),
            lines: Vec::new(),
            cols: Vec::new(),
            consts: Vec::new(),
            members: Vec::new(),
            pats: Vec::new(),
            fmts: Vec::new(),
            struct_lits: Vec::new(),
            defaults: Vec::new(),
            try_targets: Vec::new(),
            ret_error: None,
            enum_variants: Vec::new(),
            casts: Vec::new(),
            coerces: Vec::new(),
            paths: Vec::new(),
            names: Vec::new(),
            children: Vec::new(),
            child_caps: Vec::new(),
            child_partial: Vec::new(),
            child_moves: Vec::new(),
            upvalues: Vec::new(),
            mutable_locals: HashSet::new(),
            cell_names: HashSet::new(),
            binding_sites: Vec::new(),
            borrow_params: HashSet::new(),
            ref_locals: HashSet::new(),
            drop_exempt: HashSet::new(),
            owning_iters: HashSet::new(),
            aliases: HashMap::default(),
            alias_log: Vec::new(),
            alias_marks: Vec::new(),
            block_consts: HashSet::new(),
            scopes: vec![HashMap::default()],
            scope_order: vec![Vec::new()],
            drop_lists: Vec::new(),
            lent_params: Vec::new(),
            lent_writebacks: Vec::new(),
            reg_top: 0,
            max_reg: 0,
            num_params: 0,
            param_types: Vec::new(),
            name,
            generics: Vec::new(),
            call_type_args: Vec::new(),
            ret_cast: None,
            guard_temps: Vec::new(),
            owned_temps: Vec::new(),
            unwind_temps: Vec::new(),
            tail_call: false,
            guard_regs: HashSet::new(),
            has_guards: false,
        }
    }

    pub(super) fn local_reg(&self, name: &str) -> Option<Reg> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    pub(super) fn upvalue_index(&self, name: &str) -> Option<u16> {
        self.upvalues.iter().position(|(n, _)| n == name).map(idx16)
    }

    /// Inserted rather than reserved, because a binding the capture scan missed compiles long
    /// before the closure that makes its capture mutable. Jump targets past an insertion shift
    /// with it and never point at the inserted op. A binding the scan found moves its value
    /// into a fresh cell, one found late only forgets the old cell and keeps the register.
    pub(super) fn insert_cell_drops(&mut self) -> Result<()> {
        let mut sites: Vec<BindingSite> = self
            .binding_sites
            .iter()
            .copied()
            .filter(|site| self.mutable_locals.contains(&site.reg))
            .collect();
        if sites.is_empty() {
            return Ok(());
        }
        sites.sort_unstable_by_key(|site| (site.at, site.reg));
        let mut code = Vec::with_capacity(self.code.len() + sites.len());
        let mut lines = Vec::with_capacity(self.lines.len() + sites.len());
        let mut cols = Vec::with_capacity(self.cols.len() + sites.len());
        // 1 entry longer than the code so a jump to the end remaps too
        let mut moved = Vec::with_capacity(self.code.len() + 1);
        let mut next = 0;
        for (at, op) in take(&mut self.code).into_iter().enumerate() {
            while sites.get(next).is_some_and(|site| site.at == at) {
                let cell = sites[next].reg;
                code.push(if sites[next].cell {
                    Op::MakeCell { cell }
                } else {
                    Op::DropCell { cell }
                });
                lines.push(self.lines[at]);
                cols.push(self.cols[at]);
                next += 1;
            }
            moved.push(u32::try_from(code.len())?);
            code.push(op);
            lines.push(self.lines[at]);
            cols.push(self.cols[at]);
        }
        moved.push(u32::try_from(code.len())?);
        retarget_jumps(&mut code, &moved);
        for (ip, _, _) in &mut self.lent_writebacks {
            *ip = moved[*ip as usize];
        }
        self.code = code;
        self.lines = lines;
        self.cols = cols;
        Ok(())
    }

    /// Drops the ops flagged in `dead` and retargets every jump.
    pub(super) fn remove_ops(&mut self, dead: &[bool]) -> Result<()> {
        if !dead.iter().any(|d| *d) {
            return Ok(());
        }
        let mut code = Vec::with_capacity(self.code.len());
        let mut lines = Vec::with_capacity(self.lines.len());
        let mut cols = Vec::with_capacity(self.cols.len());
        let mut moved = Vec::with_capacity(self.code.len() + 1);
        for (at, op) in take(&mut self.code).into_iter().enumerate() {
            // a jump to a removed op lands on the op after it
            moved.push(u32::try_from(code.len())?);
            if dead[at] {
                continue;
            }
            code.push(op);
            lines.push(self.lines[at]);
            cols.push(self.cols[at]);
        }
        moved.push(u32::try_from(code.len())?);
        retarget_jumps(&mut code, &moved);
        for (ip, _, _) in &mut self.lent_writebacks {
            *ip = moved[*ip as usize];
        }
        self.code = code;
        self.lines = lines;
        self.cols = cols;
        Ok(())
    }

    /// A borrow parameter or a reference local forwards a handle, never a value of its own.
    pub(super) fn shares_only(&self, reg: Reg) -> bool {
        self.borrow_params.contains(&reg) || self.ref_locals.contains(&reg)
    }

    pub(super) fn into_chunk(mut self, file: std::sync::Arc<str>) -> Result<Chunk> {
        self.insert_cell_drops()?;
        self.child_moves = self
            .children
            .iter()
            .map(|_| Arc::from(Vec::new()))
            .collect();
        self.resolve_owns();
        let dead = self.dead_unit_loads();
        self.remove_ops(&dead)?;
        let dead = self.dead_jumps();
        self.remove_ops(&dead)?;
        // An unwind runs every drop list in the order it was made, each backwards like
        // `DropScope` runs it. An inner statement or scope ends before the one around it, so
        // its list comes first, and a temporary made later sits later in its statement's list.
        // The owned operands of a call sit between the lists. A scope that opened and closed
        // while the later operands were built, a match arm or a block, is inside the call and
        // unwinds before the operand, so its lists stay ahead of it. Every list made after the
        // op ran is outside, the temporaries of the statement included, and real Rust drops
        // the operand it moved into the call before those.
        let lists = self.drop_lists.len();
        let mut operands: Vec<(Reg, usize)> = Vec::new();
        for &(reg, until) in &self.unwind_temps {
            let until = until.min(lists);
            match operands.iter_mut().find(|(seen, _)| *seen == reg) {
                // a register serves one call at a time, and the lists before the later call
                // hold unit by then, so the later place is right for both
                Some(entry) => entry.1 = entry.1.max(until),
                None => operands.push((reg, until)),
            }
        }
        operands.sort_unstable_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
        let held: HashSet<Reg> = operands.iter().map(|(reg, _)| *reg).collect();
        let mut pending = operands.into_iter().peekable();
        let mut droppable: Vec<Reg> = Vec::new();
        for (index, list) in self.drop_lists.iter().enumerate() {
            while let Some((reg, _)) = pending.next_if(|(_, until)| *until <= index) {
                droppable.push(reg);
            }
            for &reg in list.iter().rev() {
                if held.contains(&reg) {
                    continue;
                }
                // a local sits in the list of every early `return` before its scope's own
                // list, and the scope end is the place that orders it after the later temporaries
                if let Some(seen) = droppable.iter().position(|r| *r == reg) {
                    droppable.remove(seen);
                }
                droppable.push(reg);
            }
        }
        droppable.extend(pending.map(|(reg, _)| reg));
        Ok(Chunk {
            code: self.code,
            lines: self.lines,
            cols: self.cols,
            file,
            num_regs: self.max_reg as usize,
            num_params: self.num_params,
            param_types: self.param_types,
            name: self.name,
            module: 0,
            moves: false,
            consts: self.consts,
            members: self.members,
            pats: self.pats,
            fmts: self.fmts,
            struct_lits: self.struct_lits,
            enum_variants: self.enum_variants,
            casts: self.casts,
            defaults: self.defaults,
            try_targets: self.try_targets,
            coerces: self.coerces,
            paths: self.paths,
            names: self.names,
            children: self.children,
            child_caps: self.child_caps,
            child_moves: self.child_moves,
            generics: self.generics,
            drop_lists: self.drop_lists,
            droppable: droppable.into(),
            lent_params: self.lent_params.into(),
            lent_writebacks: self.lent_writebacks.into(),
            call_type_args: self.call_type_args,
            path_forwarder: false,
            clears_frame: self.has_guards,
        })
    }
}
