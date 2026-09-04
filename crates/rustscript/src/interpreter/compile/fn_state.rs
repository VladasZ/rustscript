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
    /// filled by the liveness pass, see `liveness.rs`
    pub(super) child_moves: Vec<Arc<[bool]>>,
    pub(super) upvalues: Vec<(String, CapSource)>,
    pub(super) mutable_locals: HashSet<Reg>,
    /// Whether a register needs a capture cell is only known once the frame is compiled, so
    /// `into_chunk` turns these into `DropCell` ops later.
    pub(super) binding_sites: Vec<(usize, Reg)>,
    /// Reference parameters. They forward the caller's handle, so they are never moved, copied
    /// or dropped here.
    pub(super) borrow_params: HashSet<Reg>,
    /// `let r = &place` bindings, shared like a borrow parameter
    pub(super) ref_locals: HashSet<Reg>,
    /// Bindings that hold a borrowed handle, so scope end must not drop them.
    pub(super) drop_exempt: HashSet<Reg>,
    /// `let r = &mut v` aliases, access compiles as access to `v` itself
    pub(super) aliases: HashMap<String, String>,
    /// `const` and `static` items declared in a block. They are locals like a `let`, but a
    /// pattern that names one tests against its value.
    pub(super) block_consts: HashSet<String>,
    pub(super) scopes: Vec<HashMap<String, Reg>>,
    /// for scope end `Drop` runs
    pub(super) scope_order: Vec<Vec<Reg>>,
    pub(super) drop_lists: Vec<std::sync::Arc<[Reg]>>,
    /// see `Chunk::lent_params`
    pub(super) lent_params: Vec<Reg>,
    /// `borrow` results not yet released, see `release_guard_temps`
    pub(super) guard_temps: Vec<Reg>,
    /// Temporaries that own a fresh value, dropped at the end of their statement, see
    /// `drop_temps`. Only kept when the program has a `Drop` impl.
    pub(super) owned_temps: Vec<Reg>,
    /// Owned call arguments, taken by the call and so only dropped by a panic before it, see
    /// `compile_args`. Only kept when the program has a `Drop` impl.
    pub(super) unwind_temps: Vec<Reg>,
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
            child_moves: Vec::new(),
            upvalues: Vec::new(),
            mutable_locals: HashSet::new(),
            binding_sites: Vec::new(),
            borrow_params: HashSet::new(),
            ref_locals: HashSet::new(),
            drop_exempt: HashSet::new(),
            aliases: HashMap::default(),
            block_consts: HashSet::new(),
            scopes: vec![HashMap::default()],
            scope_order: vec![Vec::new()],
            drop_lists: Vec::new(),
            lent_params: Vec::new(),
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

    /// Inserted rather than reserved, because the binding compiles long before the closure that makes
    /// the capture mutable. Jump targets past an insertion shift with it and never point at the
    /// inserted op.
    pub(super) fn insert_cell_drops(&mut self) -> Result<()> {
        let mut sites: Vec<(usize, Reg)> = self
            .binding_sites
            .iter()
            .copied()
            .filter(|(_, reg)| self.mutable_locals.contains(reg))
            .collect();
        if sites.is_empty() {
            return Ok(());
        }
        sites.sort_unstable();
        let mut code = Vec::with_capacity(self.code.len() + sites.len());
        let mut lines = Vec::with_capacity(self.lines.len() + sites.len());
        let mut cols = Vec::with_capacity(self.cols.len() + sites.len());
        // 1 entry longer than the code so a jump to the end remaps too
        let mut moved = Vec::with_capacity(self.code.len() + 1);
        let mut next = 0;
        for (at, op) in take(&mut self.code).into_iter().enumerate() {
            while sites.get(next).is_some_and(|(site, _)| *site == at) {
                code.push(Op::DropCell {
                    cell: sites[next].1,
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
        let mut droppable: Vec<Reg> = self
            .drop_lists
            .iter()
            .flat_map(|list| list.iter().copied())
            .chain(self.unwind_temps.iter().copied())
            .collect();
        droppable.sort_unstable_by(|a, b| b.cmp(a));
        droppable.dedup();
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
            call_type_args: self.call_type_args,
            path_forwarder: false,
            clears_frame: self.has_guards,
        })
    }
}
