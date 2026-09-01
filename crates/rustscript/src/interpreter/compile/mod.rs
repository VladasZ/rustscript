//! Lowers the `syn` AST into register bytecode once per program. The VM never does a name lookup.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use syn::{Block, Expr, FnArg, Pat};

use super::bytecode::{
    BuiltinId, Chunk, Const, DefaultIr, EnumVariant, Member, MethodName, NO_ATOM, Op, PathRef, Reg,
    ScalarTy,
};
use super::enum_def::EnumDef;
use super::resolver::{Res, Resolver};
use super::typeir::{TypeIr, lower_cast, lower_type};

/// `moved[i]` is the new index of the op that was at `i`, one entry past the end included.
fn retarget_jumps(code: &mut [Op], moved: &[u32]) {
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
    /// every script function by name, for the inference pass. A name defined twice is absent.
    pub fn_signatures: &'r HashMap<String, syn::Signature>,
    /// `&mut self` method names. A call compiles its receiver as a place split from sharing.
    pub mut_methods: &'r HashSet<String>,
    /// Includes impls on bridge types like `impl From<Point> for String`, a path call on one is a
    /// user call even when the bridge knows the name.
    pub impl_methods: &'r HashSet<(String, String)>,
    /// atoms of the impl method names no bridge knows
    pub method_atoms: &'r HashMap<String, u32>,
    /// every impl method by type and name, for the inference pass
    pub impl_sigs: &'r HashMap<(String, String), syn::Signature>,
    /// the declared type of every const and static, for the inference pass
    pub const_types: &'r HashMap<String, syn::Type>,
    /// false skips all scope drop bookkeeping
    pub has_drop: bool,
}

struct LoopCtx {
    /// patched to the end
    breaks: Vec<usize>,
    /// `None` for a labeled block, which only `break` can leave
    continue_to: Option<usize>,
    /// for `loop { break v }`
    result: Reg,
    /// a `break` or `continue` ends every scope deeper than this first
    scope_depth: usize,
    /// `'name` on the loop
    label: Option<String>,
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
    /// the type of every expression of the body being compiled, see `infer`
    types: infer::Types,
    frames: Vec<FnState>,
    loops: Vec<LoopCtx>,
    /// stamped onto every emitted op
    cur_line: u32,
    cur_col: u32,
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
            types: infer::Types::empty(),
            frames: Vec::new(),
            loops: Vec::new(),
            cur_line: 0,
            cur_col: 0,
            shapes: Vec::new(),
        }
    }

    pub(super) fn set_line(&mut self, span: proc_macro2::Span) {
        let start = span.start();
        self.cur_line = u32::try_from(start.line).unwrap_or(u32::MAX);
        self.cur_col = u32::try_from(start.column + 1).unwrap_or(u32::MAX);
    }

    pub(super) fn resolve_path_res(&self, segs: &[String]) -> Result<Res> {
        self.ctx.resolver.resolve(self.ctx.module, segs)
    }

    pub fn compile_fn(&mut self, sig: &syn::Signature, block: &Block) -> Result<Chunk> {
        self.types = infer::infer_fn(self.ctx, sig, block);
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
                    params.push(Some(&t.pat));
                    types.push(type_head(&t.ty));
                    annotations.push(Some(&t.ty));
                    borrows.push(matches!(&*t.ty, syn::Type::Reference(_)));
                }
            }
        }
        self.cur().num_params = params.len();
        self.cur().param_types = types;
        self.bind_params(sig, &params, &annotations, &borrows)?;
        // the return type retags the tail and every early `return`
        if let syn::ReturnType::Type(_, ty) = &sig.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        self.install_return_error(&sig.output);
        let ret = self.alloc();
        self.compile_block(block, ret)?;
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

    /// Parameters occupy the first registers, in order.
    fn bind_params(
        &mut self,
        sig: &syn::Signature,
        params: &[Option<&Pat>],
        annotations: &[Option<&syn::Type>],
        borrows: &[bool],
    ) -> Result<()> {
        // a pattern param binds more registers, so every param slot is claimed before any binding
        let regs: Vec<Reg> = params.iter().map(|_| self.alloc()).collect();
        for (i, (p, reg)) in params.iter().zip(regs).enumerate() {
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
            // A `mut` by value parameter may be a `Copy` of a caller value that stays live, so it
            // owns a copy unless its type rules `Copy` out.
            let mutable = match (p, &sig.inputs[i]) {
                (None, FnArg::Receiver(r)) => {
                    matches!(r.kind, syn::ReceiverKind::Value)
                        && r.mutability.is_some()
                        && self
                            .ctx
                            .impl_type
                            .is_none_or(|ty| !self.is_non_copy_name(ty))
                }
                (Some(Pat::Ident(id)), _) => {
                    id.mutability.is_some()
                        && !borrows[i]
                        && !self.is_non_copy_annotation(annotations[i])
                }
                _ => false,
            };
            if mutable {
                self.emit(Op::Copy { dst: reg, src: reg });
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
        Ok(())
    }

    pub fn compile_const(&mut self, expr: &Expr, ty: &syn::Type) -> Result<Chunk> {
        self.types = infer::infer_const(self.ctx, expr, Some(ty));
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
        let col = self.cur_col;
        let f = self.cur();
        if let Op::Method { dst, name, .. } = &op
            && f.names[usize::from(*name)].id.is_borrow()
        {
            f.guard_temps.push(*dst);
            f.has_guards = true;
        }
        f.code.push(op);
        f.lines.push(line);
        f.cols.push(col);
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

    fn define_block_const(&mut self, name: &str, reg: Reg) {
        self.define(name, reg);
        self.cur().block_consts.insert(name.to_string());
    }

    /// A closure body sees the constants of the function that holds it.
    pub(super) fn block_const(&self, name: &str) -> bool {
        self.frames.iter().any(|f| f.block_consts.contains(name))
    }

    /// `depth` 1 is the current scope alone, a `return` uses every open scope. Scopes are not popped.
    /// Without `Drop` impls only `RefCell` guard bindings drop, everything else can stay.
    fn emit_scope_drops(&mut self, depth: usize) {
        let has_drop = self.ctx.has_drop;
        if !has_drop && self.cur().guard_regs.is_empty() {
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
                .filter(|r| {
                    let f = self.cur();
                    !f.borrow_params.contains(r)
                        && (f.guard_regs.contains(r) || (has_drop && !f.drop_exempt.contains(r)))
                })
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
            | Op::CmpJumpInt { to: t, .. }
            | Op::CmpJumpIntImm { to: t, .. }
            | Op::ForNext { to: t, .. }
            | Op::TryJump { to: t, .. } => *t = to,
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
mod frame;
mod guards;
mod infer;
mod liveness;
mod macros;
mod method;
mod names;
mod pattern;
mod place;
mod struct_lit;
mod support;
mod typed;
mod walks;

use frame::FnState;

use support::{
    FloatTy, NumericTy, bin_kind, collect_pattern_names, expr_kind, first_generic_type,
    inline_holes, int_literal, is_assign_op, macro_yields_value, numeric_annotation, parse_exprs,
    parse_matches, parse_vec_repeat, type_head,
};

impl Compiler<'_> {
    /// True when the type can not be `Copy`, a `String`, a container, a cell, a user type that
    /// does not derive `Copy`. Unknown types answer false, so they copy to be safe.
    pub(super) fn is_non_copy_annotation(&self, ty: Option<&syn::Type>) -> bool {
        let Some(ty) = ty else { return false };
        match ty {
            syn::Type::Paren(p) => self.is_non_copy_annotation(Some(&p.elem)),
            syn::Type::Group(g) => self.is_non_copy_annotation(Some(&g.elem)),
            syn::Type::Tuple(t) => t
                .elems
                .iter()
                .any(|elem| self.is_non_copy_annotation(Some(elem))),
            syn::Type::Array(a) => self.is_non_copy_annotation(Some(&a.elem)),
            syn::Type::Path(p) => {
                let Some(last) = p.path.segments.last() else {
                    return false;
                };
                let name = last.ident.to_string();
                if matches!(
                    name.as_str(),
                    "String"
                        | "Vec"
                        | "VecDeque"
                        | "HashMap"
                        | "HashSet"
                        | "BTreeMap"
                        | "BTreeSet"
                        | "Box"
                        | "Rc"
                        | "Arc"
                        | "RefCell"
                        | "Cell"
                        | "Mutex"
                        | "PathBuf"
                        | "OsString"
                ) {
                    return true;
                }
                let segs: Vec<String> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                let segs = match (segs.first().map(String::as_str), self.ctx.impl_type) {
                    (Some("Self"), Some(ty)) => vec![ty.to_string()],
                    _ => segs,
                };
                match self.resolve_path_res(&segs) {
                    Ok(Res::Struct(canon)) => self.is_non_copy_name(&canon),
                    Ok(Res::Enum(canon)) => self
                        .ctx
                        .resolver
                        .enums
                        .get(&canon)
                        .is_some_and(|e| !derives_copy(&e.attrs)),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub(super) fn is_non_copy_name(&self, canon: &str) -> bool {
        self.ctx
            .resolver
            .structs
            .get(canon)
            .is_some_and(|def| !derives_copy(&def.ast.attrs))
    }
}

pub(super) fn derives_copy(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let mut found = false;
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("Copy") {
                found = true;
            }
            Ok(())
        });
        parsed.is_ok() && found
    })
}

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
