//! Lower the `syn` AST into register bytecode. Runs once per program at load.
//! Every variable is resolved to a register slot here, so the VM never does a
//! name lookup. Control flow becomes jumps, patterns become test-and-bind ops,
//! and the common macros are lowered inline.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, bail};
use syn::punctuated::Punctuated;
use syn::{BinOp, Block, Expr, FnArg, Lit, Pat, UnOp};

use super::bytecode::{
    BinKind, BuiltinId, CapSource, Chunk, Const, EnumVariant, FmtSpec, Member, MethodName, Op,
    PatInfo, Reg, StructLit,
};
use super::numeric::IntWidth;
use super::resolver::{Res, Resolver};
use super::typeir::{CastIr, TypeIr, lower_cast, lower_type};

/// Program level facts the compiler needs, filled before any body is compiled.
pub struct Ctx<'r> {
    pub resolver: &'r Resolver,
    /// The module whose source is being compiled. Paths resolve against it.
    pub module: usize,
    /// The file the module was read from, carried into every chunk it
    /// produces so runtime error traces can name it.
    pub file: std::sync::Arc<str>,
    /// True when compiling a `#[tokio::main]` program, which lets `.await`,
    /// `tokio::spawn`, and `join!` compile instead of being rejected.
    pub async_mode: bool,
    /// Concrete target of the `impl` whose method is being compiled.
    pub impl_type: Option<&'r str>,
}

/// Per function compilation state. A stack of these supports nested closures.
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
    coerces: Vec<TypeIr>,
    paths: Vec<(Vec<String>, Option<TypeIr>)>,
    names: Vec<MethodName>,
    children: Vec<Rc<Chunk>>,
    child_caps: Vec<Vec<CapSource>>,
    upvalues: Vec<(String, CapSource)>,
    mutable_locals: HashSet<Reg>,
    scopes: Vec<HashMap<String, Reg>>,
    reg_top: Reg,
    max_reg: Reg,
    num_params: usize,
    name: String,
    generics: Vec<Rc<str>>,
    call_type_args: Vec<Arc<[TypeIr]>>,
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
            enum_variants: Vec::new(),
            casts: Vec::new(),
            coerces: Vec::new(),
            paths: Vec::new(),
            names: Vec::new(),
            children: Vec::new(),
            child_caps: Vec::new(),
            upvalues: Vec::new(),
            mutable_locals: HashSet::new(),
            scopes: vec![HashMap::default()],
            reg_top: 0,
            max_reg: 0,
            num_params: 0,
            name,
            generics: Vec::new(),
            call_type_args: Vec::new(),
        }
    }

    fn local_reg(&self, name: &str) -> Option<Reg> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn upvalue_index(&self, name: &str) -> Option<u16> {
        self.upvalues
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| i as u16)
    }

    fn into_chunk(self, file: std::sync::Arc<str>) -> Chunk {
        Chunk {
            code: self.code,
            lines: self.lines,
            file,
            num_regs: self.max_reg as usize,
            num_params: self.num_params,
            name: self.name,
            module: 0,
            consts: self.consts,
            members: self.members,
            pats: self.pats,
            fmts: self.fmts,
            struct_lits: self.struct_lits,
            enum_variants: self.enum_variants,
            casts: self.casts,
            coerces: self.coerces,
            paths: self.paths,
            names: self.names,
            children: self.children,
            child_caps: self.child_caps,
            generics: self.generics,
            call_type_args: self.call_type_args,
        }
    }
}

/// A loop target for `break` and `continue`.
struct LoopCtx {
    /// Jump indices that break out, patched to the end.
    breaks: Vec<usize>,
    /// Instruction index a `continue` jumps to.
    continue_to: usize,
    /// Register holding the loop value, for `loop { break v }`.
    result: Reg,
}

pub struct Compiler<'a> {
    ctx: &'a Ctx<'a>,
    frames: Vec<FnState>,
    loops: Vec<LoopCtx>,
    /// Source line of the expression being lowered, stamped onto every emitted
    /// op so runtime errors can point at the failing line.
    cur_line: u32,
    /// A `let x: T = from_str(..)...` annotation waiting to attach to that
    /// exact `from_str` call, keyed by the call's address so a nested call
    /// inside its arguments cannot steal it. Lets the typed json path run
    /// without a turbofish.
    pub(super) json_let: Option<(*const syn::ExprCall, TypeIr)>,
    /// A `let s: String = ...collect()` annotation waiting to attach to that
    /// exact `collect` call, keyed by the call's address like `json_let`.
    /// Lets an annotated let collect into a String without a turbofish.
    pub(super) string_let: Option<*const syn::ExprMethodCall>,
}

/// Where a referenced name lives.
#[derive(Clone, Copy)]
enum NameLoc {
    Local(Reg),
    Cell(Reg),
    Upvalue(u16),
    /// Not a variable, so a function, enum variant, or other path value.
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
            string_let: None,
        }
    }

    /// Remember the line an AST node starts on, for the ops it lowers to.
    pub(super) fn set_line(&mut self, span: proc_macro2::Span) {
        self.cur_line = span.start().line as u32;
    }

    /// Resolve a path against the module being compiled.
    pub(super) fn resolve_path_res(&self, segs: &[String]) -> Result<Res> {
        self.ctx.resolver.resolve(self.ctx.module, segs)
    }

    /// Compile a top level function or a method body into a chunk.
    pub fn compile_fn(&mut self, sig: &syn::Signature, block: &Block) -> Result<Chunk> {
        self.frames.push(FnState::new(sig.ident.to_string()));
        // Record generic parameter names so a caller's turbofish type args can
        // be bound to them when the body resolves a type, e.g. `from_str::<T>`.
        let generics: Vec<Rc<str>> = sig
            .generics
            .type_params()
            .map(|p| Rc::from(p.ident.to_string().as_str()))
            .collect();
        self.cur().generics = generics;
        // Parameters occupy the first registers, self first if present.
        let mut params: Vec<Option<&Pat>> = Vec::new();
        for input in &sig.inputs {
            match input {
                FnArg::Receiver(_) => params.push(None),
                FnArg::Typed(t) => params.push(Some(&t.pat)),
            }
        }
        self.cur().num_params = params.len();
        for (i, p) in params.iter().enumerate() {
            let reg = self.alloc();
            debug_assert_eq!(reg as usize, i);
            match p {
                None => self.define("self", reg),
                Some(Pat::Ident(id)) if id.subpat.is_none() => {
                    self.define(&id.ident.to_string(), reg);
                }
                Some(pat) => self.bind_pattern_irrefutable(pat, reg)?,
            }
        }
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        Ok(self.finish_chunk())
    }

    /// Compile a const or static initializer expression into a chunk.
    pub fn compile_const(&mut self, expr: &Expr) -> Result<Chunk> {
        self.frames.push(FnState::new("<const>".to_string()));
        let ret = self.alloc();
        self.compile_into(ret, expr)?;
        self.emit(Op::Ret { src: ret });
        Ok(self.finish_chunk())
    }

    fn finish_chunk(&mut self) -> Chunk {
        let mut chunk = self.frames.pop().unwrap().into_chunk(self.ctx.file.clone());
        chunk.module = self.ctx.module as u16;
        chunk
    }

    // -- frame helpers -----------------------------------------------------

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
        self.cur().scopes.push(HashMap::default());
    }

    fn pop_scope(&mut self) {
        self.cur().scopes.pop();
    }

    fn define(&mut self, name: &str, reg: Reg) {
        self.cur()
            .scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), reg);
    }

    fn add_const(&mut self, c: Const) -> u16 {
        let f = self.cur();
        f.consts.push(c);
        (f.consts.len() - 1) as u16
    }

    fn add_member(&mut self, m: Member) -> u16 {
        let f = self.cur();
        f.members.push(m);
        (f.members.len() - 1) as u16
    }

    fn add_cast(&mut self, ty: &syn::Type) -> u16 {
        let f = self.cur();
        f.casts.push(lower_cast(ty));
        (f.casts.len() - 1) as u16
    }

    /// Lower an annotated or turbofish type against the module being compiled
    /// and the generics of the function being compiled. A closure body has no
    /// generics of its own, matching the empty type environment its frame
    /// runs under.
    pub(super) fn lower_ir(&self, ty: &syn::Type) -> TypeIr {
        let generics = &self.frames.last().unwrap().generics;
        lower_type(ty, self.ctx.resolver, self.ctx.module, generics)
    }

    fn add_coerce(&mut self, ir: TypeIr) -> u16 {
        let f = self.cur();
        f.coerces.push(ir);
        (f.coerces.len() - 1) as u16
    }

    fn add_name(&mut self, name: String) -> u16 {
        let f = self.cur();
        f.names.push(MethodName {
            id: BuiltinId::resolve(&name),
            text: name,
        });
        (f.names.len() - 1) as u16
    }

    fn add_path(&mut self, segs: Vec<String>, coerce: Option<TypeIr>) -> u16 {
        let f = self.cur();
        f.paths.push((segs, coerce));
        (f.paths.len() - 1) as u16
    }

    fn add_enum_variant(&mut self, variant: EnumVariant) -> u16 {
        let variants = &mut self.cur().enum_variants;
        if let Some(index) = variants.iter().position(|known| {
            known.enum_name == variant.enum_name && known.variant == variant.variant
        }) {
            return index as u16;
        }
        variants.push(variant);
        (variants.len() - 1) as u16
    }

    fn enum_variant(
        &self,
        enum_name: &Rc<str>,
        rest: &[String],
        fields: impl Fn(&syn::Fields) -> bool,
    ) -> Option<EnumVariant> {
        let variant_name = rest.first().filter(|_| rest.len() == 1)?;
        let definition = self.ctx.resolver.enums.get(enum_name)?;
        let variant = definition
            .variants
            .iter()
            .find(|variant| variant.ident == variant_name && fields(&variant.fields))?;
        Some(EnumVariant {
            enum_name: enum_name.clone(),
            variant: Rc::from(variant.ident.to_string()),
        })
    }

    // -- name resolution ---------------------------------------------------

    fn resolve(&mut self, name: &str) -> NameLoc {
        let depth = self.frames.len() - 1;
        if let Some(reg) = self.frames[depth].local_reg(name) {
            return if self.frames[depth].mutable_locals.contains(&reg) {
                NameLoc::Cell(reg)
            } else {
                NameLoc::Local(reg)
            };
        }
        if let Some(idx) = self.frames[depth].upvalue_index(name) {
            return NameLoc::Upvalue(idx);
        }
        match self.capture(depth, name) {
            Some(idx) => NameLoc::Upvalue(idx),
            None => NameLoc::None,
        }
    }

    /// Capture `name` into frame `depth` as an upvalue, pulling it up the chain.
    fn capture(&mut self, depth: usize, name: &str) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        if let Some(reg) = self.frames[parent].local_reg(name) {
            let source = if self.frames[parent].mutable_locals.contains(&reg) {
                CapSource::MutableLocal(reg)
            } else {
                CapSource::Local(reg)
            };
            return Some(self.add_upvalue(depth, name, source));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            let source = if self.frames[parent].upvalues[idx as usize].1.is_mutable() {
                CapSource::MutableUpvalue(idx)
            } else {
                CapSource::Upvalue(idx)
            };
            return Some(self.add_upvalue(depth, name, source));
        }
        let idx = self.capture(parent, name)?;
        let source = if self.frames[parent].upvalues[idx as usize].1.is_mutable() {
            CapSource::MutableUpvalue(idx)
        } else {
            CapSource::Upvalue(idx)
        };
        Some(self.add_upvalue(depth, name, source))
    }

    fn resolve_for_write(&mut self, name: &str) -> NameLoc {
        let depth = self.frames.len() - 1;
        if let Some(reg) = self.frames[depth].local_reg(name) {
            return if self.frames[depth].mutable_locals.contains(&reg) {
                NameLoc::Cell(reg)
            } else {
                NameLoc::Local(reg)
            };
        }
        if let Some(idx) = self.frames[depth].upvalue_index(name) {
            self.mark_upvalue_mutable(depth, idx);
            return NameLoc::Upvalue(idx);
        }
        match self.capture_mutable(depth, name) {
            Some(idx) => NameLoc::Upvalue(idx),
            None => NameLoc::None,
        }
    }

    fn capture_mutable(&mut self, depth: usize, name: &str) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        if let Some(reg) = self.frames[parent].local_reg(name) {
            self.frames[parent].mutable_locals.insert(reg);
            return Some(self.add_upvalue(depth, name, CapSource::MutableLocal(reg)));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            self.mark_upvalue_mutable(parent, idx);
            return Some(self.add_upvalue(depth, name, CapSource::MutableUpvalue(idx)));
        }
        let idx = self.capture_mutable(parent, name)?;
        Some(self.add_upvalue(depth, name, CapSource::MutableUpvalue(idx)))
    }

    fn mark_upvalue_mutable(&mut self, depth: usize, idx: u16) {
        let source = self.frames[depth].upvalues[idx as usize].1;
        let mutable_source = match source {
            CapSource::Local(reg) => {
                self.frames[depth - 1].mutable_locals.insert(reg);
                CapSource::MutableLocal(reg)
            }
            CapSource::Upvalue(parent_idx) => {
                self.mark_upvalue_mutable(depth - 1, parent_idx);
                CapSource::MutableUpvalue(parent_idx)
            }
            CapSource::MutableLocal(_) | CapSource::MutableUpvalue(_) => return,
        };
        self.frames[depth].upvalues[idx as usize].1 = mutable_source;
    }

    fn add_upvalue(&mut self, depth: usize, name: &str, src: CapSource) -> u16 {
        if let Some(i) = self.frames[depth].upvalue_index(name) {
            return i;
        }
        self.frames[depth].upvalues.push((name.to_string(), src));
        (self.frames[depth].upvalues.len() - 1) as u16
    }

    /// Load a variable reference into a register, reading upvalues as needed.
    fn load_name(&mut self, name: &str, dst: Reg) -> Result<()> {
        match self.resolve(name) {
            NameLoc::Local(reg) => {
                if reg != dst {
                    self.emit(Op::Move { dst, src: reg });
                }
                Ok(())
            }
            NameLoc::Cell(cell) => {
                self.emit(Op::LoadCell { dst, cell });
                Ok(())
            }
            NameLoc::Upvalue(idx) => {
                self.emit(Op::LoadUpvalue { dst, idx });
                Ok(())
            }
            NameLoc::None => self.compile_resolved_value(dst, &[name.to_string()]),
        }
    }

    /// A path used as a value. Resolves consts, imported variants, and unit
    /// structs at compile time, and leaves the rest for the VM.
    pub(super) fn compile_resolved_value(&mut self, dst: Reg, segs: &[String]) -> Result<()> {
        let resolved = match self.resolve_path_res(segs) {
            Ok(r) => r,
            // A name unknown inside a user module still errors at run time,
            // matching the old single file behavior for things like `None`.
            Err(_) => Res::External(segs.to_vec()),
        };
        let path_segs = match resolved {
            Res::Const(idx) => {
                self.emit(Op::LoadGlobal { dst, idx });
                return Ok(());
            }
            Res::Struct(c) => vec![c.to_string()],
            Res::Enum(c) => vec![c.to_string()],
            Res::TypeMember(c, rest) => {
                if let Some(variant) =
                    self.enum_variant(&c, &rest, |fields| matches!(fields, syn::Fields::Unit))
                {
                    let info = self.add_enum_variant(variant);
                    self.emit(Op::LoadEnum { dst, info });
                    return Ok(());
                }
                let mut segs = vec![c.to_string()];
                segs.extend(rest);
                segs
            }
            Res::Alias(m, target) => {
                let path = match &*target {
                    syn::Type::Path(p) => p.path.clone(),
                    _ => bail!("`{}` does not name a value", segs.join("::")),
                };
                match self.ctx.resolver.resolve_struct_key(m, &path) {
                    Some(c) => vec![c.to_string()],
                    None => bail!("`{}` does not name a value", segs.join("::")),
                }
            }
            Res::Module => bail!("`{}` is a module, not a value", segs.join("::")),
            Res::Fn(_) | Res::External(_) => segs.to_vec(),
        };
        let path = self.add_path(path_segs, None);
        self.emit(Op::PathValue { dst, path });
        Ok(())
    }

    // -- blocks and statements --------------------------------------------

    pub(super) fn patch_jump(&mut self, at: usize, to: u32) {
        match &mut self.cur().code[at] {
            Op::Jump { to: t }
            | Op::JumpIfFalse { to: t, .. }
            | Op::JumpIfTrue { to: t, .. }
            | Op::CmpJump { to: t, .. }
            | Op::CmpJumpImm { to: t, .. }
            | Op::ForNext { to: t, .. } => *t = to,
            _ => panic!("patch target is not a jump"),
        }
    }
}

// -- free helpers ----------------------------------------------------------

fn is_assign_op(op: &BinOp) -> bool {
    use BinOp::*;
    matches!(
        op,
        AddAssign(_)
            | SubAssign(_)
            | MulAssign(_)
            | DivAssign(_)
            | RemAssign(_)
            | BitAndAssign(_)
            | BitOrAssign(_)
            | BitXorAssign(_)
            | ShlAssign(_)
            | ShrAssign(_)
    )
}

fn bin_kind(op: &BinOp) -> Option<BinKind> {
    use BinOp::*;
    Some(match op {
        Add(_) | AddAssign(_) => BinKind::Add,
        Sub(_) | SubAssign(_) => BinKind::Sub,
        Mul(_) | MulAssign(_) => BinKind::Mul,
        Div(_) | DivAssign(_) => BinKind::Div,
        Rem(_) | RemAssign(_) => BinKind::Rem,
        Eq(_) => BinKind::Eq,
        Ne(_) => BinKind::Ne,
        Lt(_) => BinKind::Lt,
        Le(_) => BinKind::Le,
        Gt(_) => BinKind::Gt,
        Ge(_) => BinKind::Ge,
        BitAnd(_) | BitAndAssign(_) => BinKind::BitAnd,
        BitOr(_) | BitOrAssign(_) => BinKind::BitOr,
        BitXor(_) | BitXorAssign(_) => BinKind::BitXor,
        Shl(_) | ShlAssign(_) => BinKind::Shl,
        Shr(_) | ShrAssign(_) => BinKind::Shr,
        _ => return None,
    })
}

/// The two float widths, used when a `let` annotation types a bare literal.
#[derive(Clone, Copy, Eq, PartialEq)]
enum FloatTy {
    F32,
    F64,
}

/// A numeric primitive named by a `let` annotation. The annotation is the
/// only place a bare literal's width can come from, and a non-literal init
/// retags through a runtime cast, which is a no-op on an already-typed value.
#[derive(Clone, Copy)]
enum NumericTy {
    Int(IntWidth),
    Float(FloatTy),
}

fn numeric_annotation(ty: &syn::Type) -> Option<NumericTy> {
    let syn::Type::Path(p) = ty else {
        return None;
    };
    let seg = p.path.segments.last()?;
    if !matches!(seg.arguments, syn::PathArguments::None) {
        return None;
    }
    let name = seg.ident.to_string();
    match name.as_str() {
        "f32" => Some(NumericTy::Float(FloatTy::F32)),
        "f64" => Some(NumericTy::Float(FloatTy::F64)),
        _ => IntWidth::parse(&name).map(NumericTy::Int),
    }
}

/// A plain integer literal usable as an instruction immediate, including a
/// negated one, seen through parens.
fn int_literal(e: &Expr) -> Option<i64> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse::<i64>().ok(),
            Lit::Byte(b) => Some(b.value() as i64),
            _ => None,
        },
        Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => match &*u.expr {
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => i.base10_parse::<i64>().ok().map(|v| -v),
                _ => None,
            },
            _ => None,
        },
        Expr::Paren(p) => int_literal(&p.expr),
        Expr::Group(g) => int_literal(&g.expr),
        _ => None,
    }
}

/// The first concrete generic type argument of a path segment.
pub fn first_generic_type(seg: &syn::PathSegment) -> Option<&syn::Type> {
    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                return Some(t);
            }
        }
    }
    None
}

fn collect_pattern_names(pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(id) if calls::is_unit_variant_ident(id) => {}
        Pat::Ident(id) => {
            out.push(id.ident.to_string());
            if let Some(sub) = &id.subpat {
                collect_pattern_names(&sub.1, out);
            }
        }
        Pat::Tuple(t) => t.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::TupleStruct(ts) => ts.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Slice(s) => s.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Struct(s) => s
            .fields
            .iter()
            .for_each(|f| collect_pattern_names(&f.pat, out)),
        Pat::Reference(r) => collect_pattern_names(&r.pat, out),
        Pat::Paren(p) => collect_pattern_names(&p.pat, out),
        Pat::Type(t) => collect_pattern_names(&t.pat, out),
        Pat::Or(o) => {
            // Every alternative binds the same names, walk the first.
            if let Some(first) = o.cases.first() {
                collect_pattern_names(first, out);
            }
        }
        _ => {}
    }
}

/// Identifiers used as inline `{name}` holes in a format template.
/// Whether a format hole names an identifier rather than a position.
fn is_name(arg: &str) -> bool {
    !arg.is_empty()
        && arg.parse::<usize>().is_err()
        && arg.chars().all(|c| c.is_alphanumeric() || c == '_')
        && arg
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
}

fn inline_holes(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                continue;
            }
            let mut inner = String::new();
            for ic in chars.by_ref() {
                if ic == '}' {
                    break;
                }
                inner.push(ic);
            }
            // A spec can name a variable for the width or precision, as in
            // `{:w$}`. That name is a hole too, even though it sits after the
            // colon, so the value is in scope when the template renders.
            if let Some((_, spec)) = inner.split_once(':') {
                let mut token = String::new();
                for c in spec.chars() {
                    if c.is_alphanumeric() || c == '_' {
                        token.push(c);
                        continue;
                    }
                    if c == '$' && is_name(&token) {
                        out.push(token.clone());
                    }
                    token.clear();
                }
            }
            let arg = inner.split(':').next().unwrap_or("").trim();
            if is_name(arg) {
                out.push(arg.to_string());
            }
        } else if c == '}' && chars.peek() == Some(&'}') {
            chars.next();
        }
    }
    out
}

fn macro_yields_value(mac: &syn::Macro) -> bool {
    let name = mac
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    matches!(name.as_str(), "format" | "vec" | "matches" | "dbg")
}

fn parse_exprs(mac: &syn::Macro) -> Result<Vec<Expr>> {
    Ok(mac
        .parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

fn parse_vec_repeat(input: syn::parse::ParseStream) -> syn::Result<(Expr, Expr)> {
    let value: Expr = input.parse()?;
    input.parse::<syn::Token![;]>()?;
    let count: Expr = input.parse()?;
    Ok((value, count))
}

fn parse_matches(mac: &syn::Macro) -> Result<(Expr, syn::Pat, Option<Expr>)> {
    fn inner(input: syn::parse::ParseStream) -> syn::Result<(Expr, syn::Pat, Option<Expr>)> {
        let expr: Expr = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pat = syn::Pat::parse_multi_with_leading_vert(input)?;
        let guard = if input.peek(syn::Token![if]) {
            input.parse::<syn::Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok((expr, pat, guard))
    }
    Ok(mac.parse_body_with(inner)?)
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Infer(_) => "_ placeholder",
        Expr::Let(_) => "let expression",
        Expr::TryBlock(_) => "try block",
        Expr::Yield(_) => "yield",
        Expr::Const(_) => "const block",
        Expr::Verbatim(_) => "unparsed tokens",
        _ => "this expression",
    }
}

mod calls;
mod expr;
mod macros;
