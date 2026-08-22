//! Lowers the `syn` AST into register bytecode once per program. The VM never does a name lookup.

use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use syn::{Block, Expr, FnArg, Pat};

use super::bytecode::{
    BuiltinId, CapSource, Chunk, Const, DefaultIr, EnumVariant, FmtSpec, Member, MethodName,
    NO_ATOM, Op, PatInfo, PathRef, Reg, ScalarTy, StructLit,
};
use super::enum_def::EnumDef;
use super::resolver::{Res, Resolver};
use super::typeir::{CastIr, TypeIr, lower_cast, lower_type};
use walks::{
    annotation_scalar, collect_return_target, returned_collects, returned_from_strs,
    returned_json_type,
};

/// Filled before any body is compiled.
pub struct Ctx<'r> {
    pub resolver: &'r Resolver,
    /// paths resolve against it
    pub module: usize,
    /// carried into every chunk for error traces
    pub file: std::sync::Arc<str>,
    /// lets `.await`, `tokio::spawn` and `join!` compile
    pub async_mode: bool,
    pub impl_type: Option<&'r str>,
    /// `f()` is an f32 when `fn f() -> f32` says so. A name defined twice with differing returns
    /// is absent.
    pub fn_returns: &'r HashMap<String, ScalarTy>,
    /// what `written_type` reads a helper call's type from
    pub fn_return_types: &'r HashMap<String, syn::Type>,
    /// for a generic helper whose return type its arguments state
    pub fn_signatures: &'r HashMap<String, syn::Signature>,
    /// `&mut self` method names. A call compiles its receiver as a place split from sharing.
    pub mut_methods: &'r HashSet<String>,
    /// Includes impls on bridge types like `impl From<Point> for String`, a path call on one is a
    /// user call even when the bridge knows the name.
    pub impl_methods: &'r HashSet<(String, String)>,
    /// atoms of the impl method names no bridge knows
    pub method_atoms: &'r HashMap<String, u32>,
    /// false skips all scope drop bookkeeping
    pub has_drop: bool,
}

/// A stack of these supports nested closures.
struct FnState {
    code: Vec<Op>,
    lines: Vec<u32>,
    consts: Vec<Const>,
    members: Vec<Member>,
    pats: Vec<PatInfo>,
    fmts: Vec<FmtSpec>,
    struct_lits: Vec<StructLit>,
    enum_variants: Vec<EnumVariant>,
    casts: Vec<CastIr>,
    defaults: Vec<DefaultIr>,
    try_targets: Vec<Arc<str>>,
    /// the target a `?` converts into through `From`
    ret_error: Option<Arc<str>>,
    coerces: Vec<TypeIr>,
    paths: Vec<PathRef>,
    names: Vec<MethodName>,
    children: Vec<Arc<Chunk>>,
    child_caps: Vec<Vec<CapSource>>,
    upvalues: Vec<(String, CapSource)>,
    mutable_locals: HashSet<Reg>,
    /// Whether a register needs a capture cell is only known once the frame is compiled, so
    /// `into_chunk` turns these into `DropCell` ops later.
    binding_sites: Vec<(usize, Reg)>,
    /// A mutable access through a `&T` or `&mut T` parameter must not split, the caller made it unique.
    borrow_params: HashSet<Reg>,
    /// `let r = &mut v` aliases, access compiles as access to `v` itself
    aliases: HashMap<String, String>,
    scopes: Vec<HashMap<String, Reg>>,
    /// for scope end `Drop` runs
    scope_order: Vec<Vec<Reg>>,
    drop_lists: Vec<std::sync::Arc<[Reg]>>,
    reg_top: Reg,
    max_reg: Reg,
    num_params: usize,
    param_types: Vec<Option<String>>,
    name: String,
    generics: Vec<Arc<str>>,
    call_type_args: Vec<Arc<[TypeIr]>>,
    /// Retagging on the way out keeps the declared width without a cast at every call site.
    ret_cast: Option<u16>,
}

impl FnState {
    fn new(name: String) -> FnState {
        FnState {
            code: Vec::new(),
            lines: Vec::new(),
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
            upvalues: Vec::new(),
            mutable_locals: HashSet::new(),
            binding_sites: Vec::new(),
            borrow_params: HashSet::new(),
            aliases: HashMap::default(),
            scopes: vec![HashMap::default()],
            scope_order: vec![Vec::new()],
            drop_lists: Vec::new(),
            reg_top: 0,
            max_reg: 0,
            num_params: 0,
            param_types: Vec::new(),
            name,
            generics: Vec::new(),
            call_type_args: Vec::new(),
            ret_cast: None,
        }
    }

    fn local_reg(&self, name: &str) -> Option<Reg> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn upvalue_index(&self, name: &str) -> Option<u16> {
        self.upvalues.iter().position(|(n, _)| n == name).map(idx16)
    }

    /// Inserted rather than reserved, because the binding compiles long before the closure that makes
    /// the capture mutable. Jump targets past an insertion shift with it and never point at the
    /// inserted op.
    fn insert_cell_drops(&mut self) -> Result<()> {
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
        // 1 entry longer than the code so a jump to the end remaps too
        let mut moved = Vec::with_capacity(self.code.len() + 1);
        let mut next = 0;
        for (at, op) in take(&mut self.code).into_iter().enumerate() {
            while sites.get(next).is_some_and(|(site, _)| *site == at) {
                code.push(Op::DropCell {
                    cell: sites[next].1,
                });
                lines.push(self.lines[at]);
                next += 1;
            }
            moved.push(u32::try_from(code.len())?);
            code.push(op);
            lines.push(self.lines[at]);
        }
        moved.push(u32::try_from(code.len())?);
        for op in &mut code {
            match op {
                Op::Jump { to: t }
                | Op::JumpIfFalse { to: t, .. }
                | Op::JumpIfTrue { to: t, .. }
                | Op::CmpJump { to: t, .. }
                | Op::CmpJumpImm { to: t, .. }
                | Op::ForNext { to: t, .. }
                | Op::TryJump { to: t, .. }
                | Op::LoopHead { jump: t } => *t = moved[*t as usize],
                _ => {}
            }
        }
        self.code = code;
        self.lines = lines;
        Ok(())
    }

    fn into_chunk(mut self, file: std::sync::Arc<str>) -> Result<Chunk> {
        self.insert_cell_drops()?;
        let while_rejected = self
            .code
            .iter()
            .map(|_| std::sync::atomic::AtomicU8::new(0))
            .collect();
        Ok(Chunk {
            code: self.code,
            lines: self.lines,
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
            generics: self.generics,
            drop_lists: self.drop_lists,
            call_type_args: self.call_type_args,
            path_forwarder: false,
            loop_plans: Mutex::new(HashMap::new()),
            while_plans: Mutex::new(HashMap::new()),
            while_rejected,
            fn_plan: Mutex::new(None),
            fn_rejected: std::sync::atomic::AtomicU8::new(0),
        })
    }
}

struct LoopCtx {
    /// patched to the end
    breaks: Vec<usize>,
    continue_to: usize,
    /// for `loop { break v }`
    result: Reg,
    /// a `break` or `continue` ends every scope deeper than this first
    scope_depth: usize,
}

/// Where the source states a `collect` target, the call is renamed to a target specific method.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectTarget {
    Str,
    Map,
    Set,
}

impl CollectTarget {
    pub(super) fn method_name(self) -> &'static str {
        match self {
            Self::Str => "collect_string",
            Self::Map => "collect_map",
            Self::Set => "collect_set",
        }
    }

    pub(super) fn of_type(ty: &syn::Type) -> Option<Self> {
        let syn::Type::Path(p) = ty else { return None };
        match p.path.segments.last()?.ident.to_string().as_str() {
            "String" => Some(Self::Str),
            "HashMap" | "BTreeMap" => Some(Self::Map),
            "HashSet" | "BTreeSet" => Some(Self::Set),
            _ => None,
        }
    }
}

pub struct Compiler<'a> {
    ctx: &'a Ctx<'a>,
    frames: Vec<FnState>,
    loops: Vec<LoopCtx>,
    /// stamped onto every emitted op
    cur_line: u32,
    /// A `let x: T = from_str(..)` annotation keyed by the call's address, so a nested call can't
    /// steal it.
    pub(super) json_let: Option<(*const syn::ExprCall, TypeIr)>,
    /// a `let s: String = ...collect()` annotation, keyed like `json_let`
    pub(super) collect_let: Option<(*const syn::ExprMethodCall, CollectTarget)>,
    /// Every `collect` the function hands back under a `-> String`, map or set signature. A map
    /// because a tail `if` returns from several sites.
    pub(super) collect_tails: HashMap<*const syn::ExprMethodCall, CollectTarget>,
    /// `collect_tails` for `from_str`
    pub(super) json_tails: HashMap<*const syn::ExprCall, TypeIr>,
    /// an `unwrap_or_default` unwrapped again, so its default is `None`
    pub(super) option_result: Option<*const syn::ExprMethodCall>,
    /// a `let x: T = ...unwrap_or_default()` annotation
    pub(super) default_let: Option<(*const syn::ExprMethodCall, ScalarTy)>,
    /// a bare `Default::default()` and the type its context states
    pub(super) default_calls: HashMap<*const syn::ExprCall, DefaultIr>,
    /// locals annotated `Option<T>`, `Result<T, _>` or `Vec<T>`, only ever read to pick a `Default`
    pub(super) typed_locals: HashMap<String, ScalarTy>,
    /// for `written_type`
    pub(super) typed_local_types: HashMap<String, syn::Type>,
    /// Closure parameters bound to the element type. Interior mutability keeps the `written_type`
    /// walk on `&self`.
    pub(super) closure_param_types: std::cell::RefCell<HashMap<String, syn::Type>>,
    /// the same as written
    pub(super) default_let_ty: Option<(*const syn::ExprMethodCall, syn::Type)>,
    /// a `let x: T = ...sum()` annotation, the width the reduction runs in
    pub(super) reduce_let: Option<(*const syn::ExprMethodCall, ScalarTy)>,
    /// a `let x: T = v.into()` annotation
    pub(super) into_let: Option<(*const syn::ExprMethodCall, Arc<str>)>,
    /// every bare `sum`, `product` or `unwrap_or_default` handed back, mapped to the return type
    pub(super) return_tails: HashMap<*const syn::ExprMethodCall, syn::Type>,
    /// Tails and elements an annotation types ahead of compilation, so a bare literal adopts the
    /// width instead of existing as an i64 first.
    pub(super) numeric_hints: HashMap<*const Expr, NumericTy>,
    /// a `vec![..]` body is parsed only when it compiles, so the hint waits on the macro
    pub(super) vec_hints: HashMap<*const syn::Macro, syn::Type>,
    /// 1 shape per layout, so shape identity means layout identity. The member slot cache of the
    /// scalar plan keys on it.
    pub(super) shapes: Vec<std::sync::Arc<crate::interpreter::bytecode::StructShape>>,
}

#[derive(Clone, Copy)]
enum NameLoc {
    Local(Reg),
    Cell(Reg),
    Upvalue(u16),
    /// not a variable
    None,
}

impl<'a> Compiler<'a> {
    pub fn new(ctx: &'a Ctx<'a>) -> Compiler<'a> {
        Compiler {
            ctx,
            frames: Vec::new(),
            loops: Vec::new(),
            cur_line: 0,
            json_let: None,
            collect_let: None,
            collect_tails: HashMap::new(),
            json_tails: HashMap::new(),
            option_result: None,
            default_let: None,
            default_calls: HashMap::new(),
            typed_locals: HashMap::new(),
            typed_local_types: HashMap::new(),
            closure_param_types: std::cell::RefCell::new(HashMap::new()),
            default_let_ty: None,
            reduce_let: None,
            into_let: None,
            return_tails: HashMap::new(),
            numeric_hints: HashMap::new(),
            vec_hints: HashMap::new(),
            shapes: Vec::new(),
        }
    }

    pub(super) fn set_line(&mut self, span: proc_macro2::Span) {
        self.cur_line = u32::try_from(span.start().line).unwrap_or(u32::MAX);
    }

    pub(super) fn resolve_path_res(&self, segs: &[String]) -> Result<Res> {
        self.ctx.resolver.resolve(self.ctx.module, segs)
    }

    pub fn compile_fn(&mut self, sig: &syn::Signature, block: &Block) -> Result<Chunk> {
        self.frames.push(FnState::new(sig.ident.to_string()));
        // so a caller's turbofish type args can be bound, `from_str::<T>`
        let generics: Vec<Arc<str>> = sig
            .generics
            .type_params()
            .map(|p| Arc::from(p.ident.to_string().as_str()))
            .collect();
        self.cur().generics = generics;
        // parameters occupy the first registers
        let mut params: Vec<Option<&Pat>> = Vec::new();
        let mut types: Vec<Option<String>> = Vec::new();
        let mut annotations: Vec<Option<&syn::Type>> = Vec::new();
        // a mutable access through a borrow must reach the caller's storage, so it never splits
        let mut borrows: Vec<bool> = Vec::new();
        for input in &sig.inputs {
            match input {
                FnArg::Receiver(r) => {
                    params.push(None);
                    types.push(None);
                    annotations.push(None);
                    borrows.push(matches!(r.kind, syn::ReceiverKind::Reference(..)));
                }
                FnArg::Typed(t) => {
                    // recorded like an annotated let for defaults built from the param
                    if let Pat::Ident(id) = &*t.pat
                        && let Some(declared) = annotation_scalar(&t.ty)
                    {
                        self.typed_locals.insert(id.ident.to_string(), declared);
                    }
                    params.push(Some(&t.pat));
                    types.push(type_head(&t.ty));
                    annotations.push(Some(&t.ty));
                    borrows.push(matches!(&*t.ty, syn::Type::Reference(_)));
                }
            }
        }
        self.cur().num_params = params.len();
        self.cur().param_types = types;
        for (i, p) in params.iter().enumerate() {
            let reg = self.alloc();
            debug_assert_eq!(reg as usize, i);
            if borrows[i] {
                self.cur().borrow_params.insert(reg);
            }
            match p {
                None => self.define("self", reg),
                Some(Pat::Ident(id)) if id.subpat.is_none() => {
                    self.define(&id.ident.to_string(), reg);
                }
                Some(pat) => self.bind_pattern_irrefutable(pat, reg)?,
            }
            // a numeric param retags the value, so u8 arithmetic in the body panics at the u8 bound
            if let Some(ty) = annotations[i]
                && numeric_annotation(ty).is_some()
            {
                let idx = self.add_cast(ty);
                self.emit(Op::Cast {
                    dst: reg,
                    src: reg,
                    ty: idx,
                });
            }
        }
        // the return type retags the tail and every early `return`
        if let syn::ReturnType::Type(_, ty) = &sig.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        // saved and restored so a nested item fn can't inherit the hints
        let outer_collect_tails = std::mem::take(&mut self.collect_tails);
        if let Some(target) = collect_return_target(&sig.output) {
            self.collect_tails = returned_collects(block)
                .into_iter()
                .map(|call| (call, target))
                .collect();
        }
        let outer_return_tails = take(&mut self.return_tails);
        self.install_return_hints(&sig.output, block);
        // the same for a `from_str` the body hands back
        let outer_json_tails = std::mem::take(&mut self.json_tails);
        if let Some(ty) = returned_json_type(&sig.output) {
            let ir = self.lower_ir(ty);
            self.json_tails = returned_from_strs(block)
                .into_iter()
                .map(|call| (call, ir.clone()))
                .collect();
        }
        let ret = self.alloc();
        let res = self.compile_block(block, ret);
        self.collect_tails = outer_collect_tails;
        self.json_tails = outer_json_tails;
        self.return_tails = outer_return_tails;
        res?;
        if let Some(idx) = self.cur().ret_cast {
            self.emit(Op::Cast {
                dst: ret,
                src: ret,
                ty: idx,
            });
        }
        // by value parameters drop before the frame returns
        self.emit_scope_drops(1);
        self.emit(Op::Ret { src: ret });
        self.finish_chunk()
    }

    pub fn compile_const(&mut self, expr: &Expr) -> Result<Chunk> {
        self.frames.push(FnState::new("<const>".to_string()));
        let ret = self.alloc();
        self.compile_into(ret, expr)?;
        self.emit(Op::Ret { src: ret });
        self.finish_chunk()
    }

    fn finish_chunk(&mut self) -> Result<Chunk> {
        let mut chunk = self
            .frames
            .pop()
            .unwrap()
            .into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        Ok(chunk)
    }

    // frame helpers

    fn cur(&mut self) -> &mut FnState {
        self.frames.last_mut().unwrap()
    }

    fn emit(&mut self, op: Op) {
        let line = self.cur_line;
        let f = self.cur();
        f.code.push(op);
        f.lines.push(line);
    }

    fn here(&mut self) -> usize {
        self.cur().code.len()
    }

    /// Errors when a function grows past what u32 targets can address.
    fn mark(&mut self) -> Result<u32> {
        Ok(u32::try_from(self.here())?)
    }

    fn alloc(&mut self) -> Reg {
        let f = self.cur();
        let r = f.reg_top;
        f.reg_top += 1;
        if f.reg_top > f.max_reg {
            f.max_reg = f.reg_top;
        }
        r
    }

    fn push_scope(&mut self) {
        let f = self.cur();
        f.scopes.push(HashMap::default());
        f.scope_order.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let f = self.cur();
        f.scopes.pop();
        f.scope_order.pop();
    }

    fn define(&mut self, name: &str, reg: Reg) {
        let f = self.cur();
        f.aliases.remove(name);
        f.scopes.last_mut().unwrap().insert(name.to_string(), reg);
        f.scope_order.last_mut().unwrap().push(reg);
        f.binding_sites.push((f.code.len(), reg));
    }

    /// `depth` 1 is the current scope alone, a `return` uses every open scope. Scopes are not popped.
    fn emit_scope_drops(&mut self, depth: usize) {
        if !self.ctx.has_drop {
            return;
        }
        let f = self.cur();
        let total = f.scope_order.len();
        let lists: Vec<Vec<Reg>> = f
            .scope_order
            .iter()
            .skip(total.saturating_sub(depth))
            .rev()
            .cloned()
            .collect();
        for regs in lists {
            let regs: Vec<Reg> = regs
                .into_iter()
                .filter(|r| !self.cur().borrow_params.contains(r))
                .collect();
            if regs.is_empty() {
                continue;
            }
            let f = self.cur();
            f.drop_lists.push(regs.into());
            let list = idx16(f.drop_lists.len() - 1);
            self.emit(Op::DropScope { list });
        }
    }

    fn add_const(&mut self, c: Const) -> u16 {
        let f = self.cur();
        f.consts.push(c);
        idx16(f.consts.len() - 1)
    }

    fn add_member(&mut self, m: Member) -> u16 {
        let f = self.cur();
        f.members.push(m);
        idx16(f.members.len() - 1)
    }

    fn add_cast(&mut self, ty: &syn::Type) -> u16 {
        let f = self.cur();
        f.casts.push(lower_cast(ty));
        idx16(f.casts.len() - 1)
    }

    /// A closure body has no generics of its own.
    pub(super) fn lower_ir(&self, ty: &syn::Type) -> TypeIr {
        let generics = &self.frames.last().unwrap().generics;
        lower_type(ty, self.ctx.resolver, self.ctx.module, generics)
    }

    fn add_coerce(&mut self, ir: TypeIr) -> u16 {
        let f = self.cur();
        f.coerces.push(ir);
        idx16(f.coerces.len() - 1)
    }

    fn add_name(&mut self, name: String) -> u16 {
        self.add_name_with(name, None)
    }

    fn add_name_with(&mut self, name: String, scalar: Option<ScalarTy>) -> u16 {
        self.add_name_full(name, scalar, None, false)
    }

    fn add_name_full(
        &mut self,
        name: String,
        scalar: Option<ScalarTy>,
        default: Option<DefaultIr>,
        place: bool,
    ) -> u16 {
        let bare = name.strip_prefix("r#").unwrap_or(&name);
        let id = BuiltinId::resolve(bare);
        let atom = self.ctx.method_atoms.get(bare).copied().unwrap_or(NO_ATOM);
        let f = self.cur();
        f.names.push(MethodName {
            id,
            atom,
            text: name,
            scalar,
            default: default.map(Arc::new),
            place,
        });
        idx16(f.names.len() - 1)
    }

    /// `String::from` after `impl From<Point> for String` stays a user call.
    pub(super) fn external_path(&self, segs: Vec<String>, coerce: Option<TypeIr>) -> PathRef {
        if let [.., ty, name] = segs.as_slice()
            && self.ctx.impl_methods.contains(&(ty.clone(), name.clone()))
        {
            return PathRef::user(segs, coerce);
        }
        PathRef::new(segs, coerce)
    }

    fn add_path(&mut self, path: PathRef) -> u16 {
        let f = self.cur();
        f.paths.push(path);
        idx16(f.paths.len() - 1)
    }

    fn add_enum_variant(&mut self, variant: EnumVariant) -> u16 {
        let variants = &mut self.cur().enum_variants;
        if let Some(index) = variants.iter().position(|known| {
            EnumDef::same(&known.def, &variant.def) && known.variant == variant.variant
        }) {
            return idx16(index);
        }
        variants.push(variant);
        idx16(variants.len() - 1)
    }

    fn enum_variant(
        &self,
        enum_name: &Arc<str>,
        rest: &[String],
        fields: impl Fn(&syn::Fields) -> bool,
    ) -> Option<EnumVariant> {
        let variant_name = rest.first().filter(|_| rest.len() == 1)?;
        let definition = self.ctx.resolver.enums.get(enum_name)?;
        definition
            .variants
            .iter()
            .find(|variant| variant.ident == variant_name && fields(&variant.fields))?;
        let def = self.ctx.resolver.enum_defs.get(enum_name)?;
        Some(EnumVariant {
            def: def.clone(),
            variant: def.variant_index(variant_name)?,
        })
    }

    // name resolution

    pub(super) fn patch_jump(&mut self, at: usize, to: u32) {
        match &mut self.cur().code[at] {
            Op::Jump { to: t }
            | Op::JumpIfFalse { to: t, .. }
            | Op::JumpIfTrue { to: t, .. }
            | Op::CmpJump { to: t, .. }
            | Op::CmpJumpImm { to: t, .. }
            | Op::ForNext { to: t, .. }
            | Op::TryJump { to: t, .. }
            | Op::LoopHead { jump: t } => *t = to,
            _ => panic!("patch target is not a jump"),
        }
    }
}

// free helpers

mod assign;
mod block;
mod calls;
mod closure;
mod defaults;
mod expr;
mod flow;
mod macros;
mod method;
mod names;
mod pattern;
mod place;
mod struct_lit;
mod support;
mod walks;
mod written;
mod written_type;

use support::{
    FloatTy, NumericTy, bin_kind, collect_pattern_names, expr_kind, first_generic_type,
    inline_holes, int_literal, is_assign_op, macro_yields_value, numeric_annotation,
    numeric_target, parse_exprs, parse_matches, parse_vec_repeat, type_head,
};

/// Past u16 is a compiler bug, an abort beats a wrapped index.
pub(super) fn idx16(i: usize) -> u16 {
    u16::try_from(i).expect("bytecode table exceeds u16 indices")
}

pub(super) fn derives_default(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let mut found = false;
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("Default") {
                found = true;
            }
            Ok(())
        });
        parsed.is_ok() && found
    })
}
