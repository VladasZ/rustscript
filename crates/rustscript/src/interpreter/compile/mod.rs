//! Lower the `syn` AST into register bytecode. Runs once per program at load.
//! Every variable is resolved to a register slot here, so the VM never does a
//! name lookup. Control flow becomes jumps, patterns become test-and-bind ops,
//! and the common macros are lowered inline.

use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;
use syn::punctuated::Punctuated;
use syn::{BinOp, Block, Expr, FnArg, Lit, Pat, UnOp};

use super::bytecode::{
    BinKind, BuiltinId, CapSource, Chunk, Const, DefaultIr, EnumVariant, FmtSpec, Member,
    MethodName, NO_ATOM, NO_CONV, Op, PatInfo, PathRef, Reg, ScalarTy, StructLit, StructShape,
};
use super::enum_def::EnumDef;
use super::numeric::IntWidth;
use super::resolver::{Res, Resolver};
use super::typeir::{CastIr, TypeIr, lower_cast, lower_type};
use expr::{
    annotation_scalar, collect_return_target, returned_collects, returned_exprs,
    returned_from_strs, returned_json_type,
};

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
    /// Stated return scalars of the script's own functions, one more place a
    /// `Default` payload is written down: `f()` is an f32 when `fn f() -> f32`
    /// says so. A name defined more than once with differing returns is
    /// absent, since the call site cannot tell which one it reaches.
    pub fn_returns: &'r HashMap<String, ScalarTy>,
    /// The full written return type of each uniquely named function, the
    /// source `written_type` reads a helper call's type from.
    pub fn_return_types: &'r HashMap<String, syn::Type>,
    /// The whole signature of each uniquely named function, for a generic
    /// helper whose return type is a parameter its arguments state.
    pub fn_signatures: &'r HashMap<String, syn::Signature>,
    /// Names of user methods any impl declares with a `&mut self` receiver.
    /// A call to one of these compiles its receiver as a place, split from
    /// value sharing first, so the mutation stays private to the receiver.
    pub mut_methods: &'r HashSet<String>,
    /// Every `(type, method)` an impl block declares, including impls on
    /// bridge type names like `impl From<Point> for String`. A path call on
    /// one of these is a user call even when the bridge knows the name.
    pub impl_methods: &'r HashSet<(String, String)>,
    /// The atoms of the impl method names no bridge knows, so a call site
    /// carries the id its receiver's type is looked up by.
    pub method_atoms: &'r HashMap<String, u32>,
    /// Whether any type in the program has a `Drop` impl. False skips all
    /// scope-drop bookkeeping, the common case pays nothing.
    pub has_drop: bool,
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
    defaults: Vec<DefaultIr>,
    try_targets: Vec<Arc<str>>,
    /// The user error type of this function's `Result` return, the target
    /// a `?` converts into through `From`.
    ret_error: Option<Arc<str>>,
    coerces: Vec<TypeIr>,
    paths: Vec<PathRef>,
    names: Vec<MethodName>,
    children: Vec<Arc<Chunk>>,
    child_caps: Vec<Vec<CapSource>>,
    upvalues: Vec<(String, CapSource)>,
    mutable_locals: HashSet<Reg>,
    /// Every binding site in the frame, as the code position the binding
    /// takes effect at and the register it binds. Whether a register needs
    /// a capture cell is only known once the whole frame is compiled, so the
    /// sites are collected here and `into_chunk` turns the ones that do into
    /// `DropCell` ops.
    binding_sites: Vec<(usize, Reg)>,
    /// Parameters that arrived as `&T` or `&mut T`. A mutable access through
    /// one must not split its storage, the caller's place shares it and the
    /// caller made it unique before the call.
    borrow_params: HashSet<Reg>,
    /// `let r = &mut v` bindings, name to borrowed name. Access through the
    /// alias compiles as access to the borrowed variable itself.
    aliases: HashMap<String, String>,
    scopes: Vec<HashMap<String, Reg>>,
    /// Bindings per scope in declaration order, for scope-end `Drop` runs.
    scope_order: Vec<Vec<Reg>>,
    drop_lists: Vec<std::sync::Arc<[Reg]>>,
    reg_top: Reg,
    max_reg: Reg,
    num_params: usize,
    param_types: Vec<Option<String>>,
    name: String,
    generics: Vec<Arc<str>>,
    call_type_args: Vec<Arc<[TypeIr]>>,
    /// The cast every return value of this frame passes through, when the
    /// signature declares a numeric scalar. Retagging on the way out keeps
    /// the declared width without a cast at every call site.
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

    /// Put a `DropCell` in front of every binding of a mutably captured
    /// local. The op is inserted rather than reserved up front, because the
    /// binding compiles long before the closure that makes the capture
    /// mutable, so every jump target past an insertion shifts with it. A
    /// jump keeps pointing at the op it always pointed at, never at an
    /// inserted `DropCell`, so only the fall through from the binding above
    /// it reaches one.
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
        // Position each old op lands at, one entry longer than the code so a
        // jump to the end of the frame remaps too.
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

/// A loop target for `break` and `continue`.
struct LoopCtx {
    /// Jump indices that break out, patched to the end.
    breaks: Vec<usize>,
    /// Instruction index a `continue` jumps to.
    continue_to: usize,
    /// Register holding the loop value, for `loop { break v }`.
    result: Reg,
    /// Open scope count at loop entry. A `break` or `continue` ends every
    /// scope deeper than this, so their `Drop` impls run first.
    scope_depth: usize,
}

/// The collect targets the compiler can name at the call site. `collect` is
/// type driven in real Rust and the interpreter has no types, so where the
/// source states the target, the call is renamed to a target-specific method
/// the runtime answers directly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectTarget {
    Str,
    Map,
    Set,
}

impl CollectTarget {
    /// The runtime method name a targeted `collect` is renamed to.
    pub(super) fn method_name(self) -> &'static str {
        match self {
            Self::Str => "collect_string",
            Self::Map => "collect_map",
            Self::Set => "collect_set",
        }
    }

    /// The target a type names, when it is one collect must build specially.
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
    /// Source line of the expression being lowered, stamped onto every emitted
    /// op so runtime errors can point at the failing line.
    cur_line: u32,
    /// A `let x: T = from_str(..)...` annotation waiting to attach to that
    /// exact `from_str` call, keyed by the call's address so a nested call
    /// inside its arguments cannot steal it. Lets the typed json path run
    /// without a turbofish.
    pub(super) json_let: Option<(*const syn::ExprCall, TypeIr)>,
    /// A `let s: String = ...collect()` (or HashMap/HashSet) annotation
    /// waiting to attach to that exact `collect` call, keyed by the call's
    /// address like `json_let`. Lets an annotated let collect into a String,
    /// a map, or a set without a turbofish.
    pub(super) collect_let: Option<(*const syn::ExprMethodCall, CollectTarget)>,
    /// Every `collect` in the current function whose value the function hands
    /// back, when that function is declared `-> String`, a map, or a set. A
    /// map rather than one slot because an `if` or a `match` in tail position
    /// returns from several call sites. Keyed by address like the hints
    /// above, so only those exact calls collect into the named target.
    pub(super) collect_tails: HashMap<*const syn::ExprMethodCall, CollectTarget>,
    /// Every `from_str` in the current function whose parsed value the function
    /// hands back, mapped to the payload type its signature names. The same
    /// idea as `collect_tails`, for the typed json path rather than `collect`.
    pub(super) json_tails: HashMap<*const syn::ExprCall, TypeIr>,
    /// An `unwrap_or_default` call whose result is unwrapped again, so it
    /// produced an `Option` and its own default is `None`. Keyed by address
    /// like the two above.
    pub(super) option_result: Option<*const syn::ExprMethodCall>,
    /// A `let x: T = ...unwrap_or_default()` annotation waiting to attach to
    /// that exact call, naming the payload the default is built from.
    pub(super) default_let: Option<(*const syn::ExprMethodCall, ScalarTy)>,
    /// A bare `Default::default()` call and the type its context states: a
    /// `let` annotation, a struct field, or the struct a `..Default::default()`
    /// completes. Keyed by address like the hints above.
    pub(super) default_calls: HashMap<*const syn::ExprCall, DefaultIr>,
    /// Declared types of locals annotated `Option<T>`, `Result<T, _>`, or
    /// `Vec<T>`, as `Opt(T)` or `List(T)`, so `opt.unwrap_or_default()` and
    /// `v.get(i).cloned().unwrap_or_default()` can build the right default
    /// from the type the binding was declared with. Only ever read to pick a
    /// `Default`.
    pub(super) typed_locals: HashMap<String, ScalarTy>,
    /// The full annotation of every annotated local, for `written_type`.
    pub(super) typed_local_types: HashMap<String, syn::Type>,
    /// A `let x: T = ...unwrap_or_default()` annotation as written, waiting
    /// to attach to that exact call.
    pub(super) default_let_ty: Option<(*const syn::ExprMethodCall, syn::Type)>,
    /// A `let x: T = ...sum()` or `...product()` annotation waiting to attach
    /// to that exact call, the width the reduction runs in.
    pub(super) reduce_let: Option<(*const syn::ExprMethodCall, ScalarTy)>,
    /// A `let x: T = v.into()` annotation naming the user type whose `From`
    /// impl the call goes through.
    pub(super) into_let: Option<(*const syn::ExprMethodCall, Arc<str>)>,
    /// Every bare `sum`, `product`, or `unwrap_or_default` whose value the
    /// current function hands back, mapped to the declared return type. The
    /// signature is the third place that type is knowable, after a turbofish
    /// and an annotated `let`.
    pub(super) return_tails: HashMap<*const syn::ExprMethodCall, syn::Type>,
    /// Expressions an annotation types ahead of their compilation, keyed by
    /// address: the tails of the branches, blocks, and arms an annotated
    /// `let` init is made of, and the elements of a `vec![..]` under a
    /// `Vec<u8>` annotation. A bare literal there adopts the width instead
    /// of existing as an i64 first.
    pub(super) numeric_hints: HashMap<*const Expr, NumericTy>,
    /// A `vec![..]` whose element type an annotation states. Its body is
    /// parsed only when it compiles, so the hint waits on the macro itself
    /// and is handed to the elements then.
    pub(super) vec_hints: HashMap<*const syn::Macro, syn::Type>,
    /// One shape per distinct literal layout in this compiler, so every
    /// instance of the same struct shares one arc and shape identity means
    /// layout identity, which the scalar plan's member slot cache keys on.
    pub(super) shapes: Vec<std::sync::Arc<crate::interpreter::bytecode::StructShape>>,
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
            collect_let: None,
            collect_tails: HashMap::new(),
            json_tails: HashMap::new(),
            option_result: None,
            default_let: None,
            default_calls: HashMap::new(),
            typed_locals: HashMap::new(),
            typed_local_types: HashMap::new(),
            default_let_ty: None,
            reduce_let: None,
            into_let: None,
            return_tails: HashMap::new(),
            numeric_hints: HashMap::new(),
            vec_hints: HashMap::new(),
            shapes: Vec::new(),
        }
    }

    /// Remember the line an AST node starts on, for the ops it lowers to.
    pub(super) fn set_line(&mut self, span: proc_macro2::Span) {
        self.cur_line = u32::try_from(span.start().line).unwrap_or(u32::MAX);
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
        let generics: Vec<Arc<str>> = sig
            .generics
            .type_params()
            .map(|p| Arc::from(p.ident.to_string().as_str()))
            .collect();
        self.cur().generics = generics;
        // Parameters occupy the first registers, self first if present.
        let mut params: Vec<Option<&Pat>> = Vec::new();
        let mut types: Vec<Option<String>> = Vec::new();
        let mut annotations: Vec<Option<&syn::Type>> = Vec::new();
        // Whether each parameter arrived by reference. A mutable access
        // through a borrow must reach the caller's storage, so it is never
        // split from sharing inside the callee.
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
                    // A param annotation is a type the program wrote down,
                    // recorded like an annotated let, so a default built from
                    // the param in the body reads the right type.
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
            // A numeric param annotation retags the incoming value, so u8
            // arithmetic in the body panics at the u8 bound exactly like
            // debug Rust even when the caller passed a bare literal.
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
        // The declared numeric return type retags every value on the way
        // out, the tail and each early `return` alike.
        if let syn::ReturnType::Type(_, ty) = &sig.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        // A `-> String` (or map or set) signature names the target of every
        // `collect` this body returns, which is the third place that target is
        // knowable after a turbofish and an annotated `let`. Saved and
        // restored so a nested item fn or a method compiled inside this one
        // cannot inherit the hints.
        let outer_collect_tails = std::mem::take(&mut self.collect_tails);
        if let Some(target) = collect_return_target(&sig.output) {
            self.collect_tails = returned_collects(block)
                .into_iter()
                .map(|call| (call, target))
                .collect();
        }
        let outer_return_tails = take(&mut self.return_tails);
        self.install_return_hints(&sig.output, block);
        // The same for a `from_str` the body hands back, whose target is the
        // payload of the return type rather than a `let` annotation.
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
        // By-value parameters die with the function, so their `Drop` impls
        // run before the frame returns.
        self.emit_scope_drops(1);
        self.emit(Op::Ret { src: ret });
        self.finish_chunk()
    }

    /// Compile a const or static initializer expression into a chunk.
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

    /// The current position as a jump target. Errors when a function grows
    /// past the op count the bytecode's u32 targets can address.
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
        // A fresh binding shadows any `&mut` alias of the same name.
        f.aliases.remove(name);
        f.scopes.last_mut().unwrap().insert(name.to_string(), reg);
        f.scope_order.last_mut().unwrap().push(reg);
        f.binding_sites.push((f.code.len(), reg));
    }

    /// Emit the `Drop` run for the innermost `depth` open scopes, innermost
    /// first, without popping them. `depth` 1 is the current scope alone;
    /// a `return` uses every open scope. Emits nothing when the program has
    /// no `Drop` impl.
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

    /// A path outside the script's own items. The bridge table names it,
    /// unless a user impl on that type name declares the method, `String::from`
    /// after `impl From<Point> for String`, which stays a user call.
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

    // -- name resolution ---------------------------------------------------

    fn resolve(&mut self, name: &str) -> NameLoc {
        let name = &self.unalias(name);
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

    /// The `&mut` alias target of `name` in an enclosing frame, for a
    /// closure dereferencing an alias it captured. A frame that defines
    /// `name` as its own local or upvalue shadows any outer alias.
    pub(super) fn enclosing_alias_target(&self, name: &str) -> Option<String> {
        for frame in self.frames.iter().rev().skip(1) {
            let mut seen = name;
            while let Some(next) = frame.aliases.get(seen) {
                seen = next;
            }
            if seen != name {
                return Some(seen.to_string());
            }
            if frame.local_reg(name).is_some() || frame.upvalue_index(name).is_some() {
                return None;
            }
        }
        None
    }

    /// Follow a parent frame's `&mut` aliases to the borrowed variable, so a
    /// closure capturing `r` from `let r = &mut v` captures `v` itself.
    fn parent_alias_target(&self, parent: usize, name: &str) -> Option<String> {
        let aliases = &self.frames[parent].aliases;
        let mut seen = aliases.get(name)?;
        while let Some(next) = aliases.get(seen) {
            seen = next;
        }
        Some(seen.clone())
    }

    /// Capture `name` into frame `depth` as an upvalue, pulling it up the chain.
    fn capture(&mut self, depth: usize, name: &str) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        // Writes through a `&mut` alias must reach the borrowed variable
        // across the frame boundary, so the capture is mutable.
        if let Some(target) = self.parent_alias_target(parent, name) {
            return self.capture_mutable_as(depth, &target, name);
        }
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
        let name = &self.unalias(name);
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
        self.capture_mutable_as(depth, name, name)
    }

    /// Like `capture_mutable`, registering the upvalue under `register_as`.
    /// The two names differ when an alias captures its borrowed variable.
    fn capture_mutable_as(&mut self, depth: usize, name: &str, register_as: &str) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        if let Some(target) = self.parent_alias_target(parent, name) {
            return self.capture_mutable_as(depth, &target, register_as);
        }
        if let Some(reg) = self.frames[parent].local_reg(name) {
            self.frames[parent].mutable_locals.insert(reg);
            return Some(self.add_upvalue(depth, register_as, CapSource::MutableLocal(reg)));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            self.mark_upvalue_mutable(parent, idx);
            return Some(self.add_upvalue(depth, register_as, CapSource::MutableUpvalue(idx)));
        }
        let idx = self.capture_mutable(parent, name)?;
        Some(self.add_upvalue(depth, register_as, CapSource::MutableUpvalue(idx)))
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
        idx16(self.frames[depth].upvalues.len() - 1)
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
        let resolved = self.resolve_path_res(segs)?;
        let path = match resolved {
            Res::Const(idx) => {
                self.emit(Op::LoadGlobal { dst, idx });
                return Ok(());
            }
            Res::Struct(c) | Res::Enum(c) => PathRef::user(vec![c.to_string()], None),
            Res::TypeMember(c, rest) => {
                if let Some(variant) =
                    self.enum_variant(&c, &rest, |fields| matches!(fields, syn::Fields::Unit))
                {
                    let info = self.add_enum_variant(variant);
                    self.emit(Op::LoadEnum { dst, info });
                    return Ok(());
                }
                // An associated const, `S::LIMIT`, registered at load as a
                // `Type::NAME` global. The consts table that holds it is the
                // impl's module, which may not be the module using it.
                if rest.len() == 1 {
                    let key = format!("{}::{}", crate::interpreter::resolver::bare(&c), rest[0]);
                    let found = self
                        .ctx
                        .resolver
                        .modules
                        .iter()
                        .find_map(|syms| syms.consts.get(&key).copied());
                    if let Some(idx) = found {
                        self.emit(Op::LoadGlobal { dst, idx });
                        return Ok(());
                    }
                }
                let mut segs = vec![c.to_string()];
                segs.extend(rest);
                PathRef::user(segs, None)
            }
            Res::Alias(m, target) => {
                let path = match &*target {
                    syn::Type::Path(p) => p.path.clone(),
                    _ => bail!("`{}` does not name a value", segs.join("::")),
                };
                match self.ctx.resolver.resolve_struct_key(m, &path) {
                    Some(c) => PathRef::user(vec![c.to_string()], None),
                    None => bail!("`{}` does not name a value", segs.join("::")),
                }
            }
            Res::Module => bail!("`{}` is a module, not a value", segs.join("::")),
            Res::External(canon) => {
                // `None`, `Ordering::Less`, and the other builtin unit
                // variants load their value in place, like a user variant.
                if let Some((def, index)) = self.resolve_variant(segs)
                    && def.is_unit(index)
                {
                    let info = self.add_enum_variant(EnumVariant {
                        def,
                        variant: index,
                    });
                    self.emit(Op::LoadEnum { dst, info });
                    return Ok(());
                }
                self.external_path(canon, None)
            }
            Res::Fn(_) => PathRef::user(segs.to_vec(), None),
        };
        let path = self.add_path(path);
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
            | Op::ForNext { to: t, .. }
            | Op::TryJump { to: t, .. }
            | Op::LoopHead { jump: t } => *t = to,
            _ => panic!("patch target is not a jump"),
        }
    }
}

// -- free helpers ----------------------------------------------------------

fn is_assign_op(op: &BinOp) -> bool {
    use BinOp::{
        AddAssign, BitAndAssign, BitOrAssign, BitXorAssign, DivAssign, MulAssign, RemAssign,
        ShlAssign, ShrAssign, SubAssign,
    };
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
    use BinOp::{
        Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div,
        DivAssign, Eq, Ge, Gt, Le, Lt, Mul, MulAssign, Ne, Rem, RemAssign, Shl, ShlAssign, Shr,
        ShrAssign, Sub, SubAssign,
    };
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
pub(super) enum FloatTy {
    F32,
    F64,
}

/// A numeric primitive named by a `let` annotation. The annotation is the
/// only place a bare literal's width can come from, and a non-literal init
/// retags through a runtime cast, which is a no-op on an already-typed value.
#[derive(Clone, Copy)]
pub(super) enum NumericTy {
    Int(IntWidth),
    Float(FloatTy),
}

/// The numeric type behind a stated scalar, for the literal hints.
pub(super) fn numeric_target(scalar: &ScalarTy) -> Option<NumericTy> {
    match scalar {
        ScalarTy::Int(width) => Some(NumericTy::Int(*width)),
        ScalarTy::F32 => Some(NumericTy::Float(FloatTy::F32)),
        ScalarTy::F64 => Some(NumericTy::Float(FloatTy::F64)),
        _ => None,
    }
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
            Lit::Byte(b) => Some(i64::from(b.value())),
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

/// The head name of a written type, references and slices peeled off, so
/// `&serde_json::Value` is `Value` and `&[String]` is `Vec`. Only the head
/// matters to the coverage check, which asks what the receiver is, not what it
/// holds. None for anything that is not a plain path.
fn type_head(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_head(&r.elem),
        syn::Type::Paren(p) => type_head(&p.elem),
        syn::Type::Group(g) => type_head(&g.elem),
        syn::Type::Slice(_) | syn::Type::Array(_) => Some("Vec".to_string()),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
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
mod place;
mod written;
mod written_type;

/// A table index as the u16 the bytecode stores. Every compiler table is
/// interned under that limit, so blowing past it is a compiler bug and an
/// immediate abort beats silently wrapped indices.
pub(super) fn idx16(i: usize) -> u16 {
    u16::try_from(i).expect("bytecode table exceeds u16 indices")
}

/// Whether a `#[derive(..)]` list names `Default`.
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

/// How deep a defaulted type may nest before lowering gives up, so a struct
/// holding a `Vec` of itself still terminates.
const DEFAULT_DEPTH: usize = 8;

impl Compiler<'_> {
    /// The default value of a written type, or `None` when the type has no
    /// `Default` this interpreter can build: a user type without the derive,
    /// a reference, a type this model does not describe.
    pub(super) fn default_ir(&mut self, ty: &syn::Type) -> Option<DefaultIr> {
        self.default_ir_at(ty, 0)
    }

    fn default_ir_at(&mut self, ty: &syn::Type, depth: usize) -> Option<DefaultIr> {
        if depth > DEFAULT_DEPTH {
            return None;
        }
        match ty {
            syn::Type::Tuple(t) if t.elems.is_empty() => Some(DefaultIr::Unit),
            syn::Type::Tuple(t) => {
                let items = t
                    .elems
                    .iter()
                    .map(|elem| self.default_ir_at(elem, depth + 1))
                    .collect::<Option<Vec<_>>>()?;
                Some(DefaultIr::Tuple(items))
            }
            syn::Type::Paren(p) => self.default_ir_at(&p.elem, depth),
            syn::Type::Group(g) => self.default_ir_at(&g.elem, depth),
            syn::Type::Path(p) => self.default_ir_path(&p.path, depth),
            _ => None,
        }
    }

    fn default_ir_path(&mut self, path: &syn::Path, depth: usize) -> Option<DefaultIr> {
        let last = path.segments.last()?.ident.to_string();
        if let Some(width) = IntWidth::parse(&last) {
            return Some(DefaultIr::Int(width));
        }
        let builtin = match last.as_str() {
            "f32" => Some(DefaultIr::F32),
            "f64" => Some(DefaultIr::F64),
            "bool" => Some(DefaultIr::Bool),
            "char" => Some(DefaultIr::Char),
            "String" | "str" => Some(DefaultIr::Str),
            "Vec" | "VecDeque" => Some(DefaultIr::Vec),
            "HashMap" | "BTreeMap" => Some(DefaultIr::Map),
            "HashSet" | "BTreeSet" => Some(DefaultIr::Set),
            "Option" => Some(DefaultIr::Opt),
            _ => None,
        };
        if builtin.is_some() {
            return builtin;
        }
        let mut segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        // `Self` inside an impl names the impl's own type.
        if segs.len() == 1
            && segs[0] == "Self"
            && let Some(ty) = self.ctx.impl_type
        {
            segs[0] = ty.to_string();
        }
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) => self.default_ir_struct(&canon, depth),
            Res::Enum(canon) => self.default_ir_enum(&canon),
            Res::Alias(m, target) => match &*target {
                syn::Type::Path(p) => {
                    let canon = self.ctx.resolver.resolve_struct_key(m, &p.path)?;
                    self.default_ir_struct(&canon, depth)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// A derived `Default` struct: one default per field, in declaration
    /// order, under the full shape of the type.
    fn default_ir_struct(&mut self, canon: &Arc<str>, depth: usize) -> Option<DefaultIr> {
        let def = self.ctx.resolver.structs.get(canon)?;
        if !derives_default(&def.ast.attrs) {
            return None;
        }
        let ast = def.ast.clone();
        let mut names = Vec::new();
        let mut renames = Vec::new();
        let mut fields = Vec::new();
        for field in &ast.fields {
            let name = field.ident.as_ref()?.to_string();
            names.push(Arc::<str>::from(name));
            renames.push(super::serde_attrs::serde_rename(field).map(Arc::<str>::from));
            fields.push(self.default_ir_at(&field.ty, depth + 1)?);
        }
        let shape = self.shape_for(canon, names, renames);
        Some(DefaultIr::Struct { shape, fields })
    }

    /// The `#[default]` unit variant of a derived `Default` enum.
    fn default_ir_enum(&mut self, canon: &Arc<str>) -> Option<DefaultIr> {
        let ast = self.ctx.resolver.enums.get(canon)?.clone();
        if !derives_default(&ast.attrs) {
            return None;
        }
        let variant = ast.variants.iter().find(|variant| {
            variant
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("default"))
        })?;
        let name = variant.ident.to_string();
        let def = self.ctx.resolver.enum_defs.get(canon)?;
        Some(DefaultIr::Enum(EnumVariant {
            def: def.clone(),
            variant: def.variant_index(&name)?,
        }))
    }

    /// The shape of a script struct over the given fields, shared with any
    /// literal of the same layout so shape identity stays layout identity.
    pub(super) fn shape_for(
        &mut self,
        name: &Arc<str>,
        fields: Vec<Arc<str>>,
        renames: Vec<Option<Arc<str>>>,
    ) -> Arc<StructShape> {
        if let Some(known) = self
            .shapes
            .iter()
            .find(|s| s.name == *name && s.fields == fields && s.renames == renames)
        {
            return known.clone();
        }
        let type_id = self.ctx.resolver.type_id_of(name);
        let built = StructShape::typed(name.clone(), type_id, fields, renames);
        self.shapes.push(built.clone());
        built
    }

    /// The derived default of a struct named by its canonical key.
    pub(super) fn default_ir_for_struct(&mut self, canon: &Arc<str>) -> Option<DefaultIr> {
        self.default_ir_struct(canon, 0)
    }

    /// `default_ir_path` for a call site that already split the path.
    pub(super) fn default_ir_path_pub(&mut self, path: &syn::Path) -> Option<DefaultIr> {
        self.default_ir_path(path, 0)
    }

    /// The two hints a return type gives the body: the user error type a `?`
    /// converts into, and the type of a bare `Default::default()` the body
    /// hands back.
    fn install_return_hints(&mut self, output: &syn::ReturnType, block: &Block) {
        let syn::ReturnType::Type(_, ty) = output else {
            return;
        };
        if let Some(canon) = self.result_error_type(ty) {
            self.cur().ret_error = Some(canon);
        }
        let calls: Vec<*const syn::ExprCall> = returned_exprs(block)
            .into_iter()
            .filter_map(|e| calls::bare_default_call(e).map(std::ptr::from_ref))
            .collect();
        if !calls.is_empty()
            && let Some(ir) = self.default_ir(ty)
        {
            for call in calls {
                self.default_calls.insert(call, ir.clone());
            }
        }
        let reductions = returned_exprs(block).into_iter().filter_map(|e| match e {
            Expr::MethodCall(m)
                if m.turbofish.is_none()
                    && matches!(
                        m.method.to_string().as_str(),
                        "sum" | "product" | "unwrap_or_default"
                    ) =>
            {
                Some((std::ptr::from_ref(m), (**ty).clone()))
            }
            _ => None,
        });
        self.return_tails.extend(reductions);
    }

    /// The user type `E` of a written `Result<T, E>`, when `E` is a struct
    /// or enum of the script.
    fn result_error_type(&self, ty: &syn::Type) -> Option<Arc<str>> {
        let syn::Type::Path(p) = ty else { return None };
        let last = p.path.segments.last()?;
        if last.ident != "Result" {
            return None;
        }
        let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
            return None;
        };
        let err = args
            .args
            .iter()
            .filter_map(|arg| match arg {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .nth(1)?;
        let syn::Type::Path(err_path) = err else {
            return None;
        };
        let segs: Vec<String> = err_path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) | Res::Enum(canon) => Some(canon),
            _ => None,
        }
    }

    /// The canonical key of a user struct or enum named by a type path.
    pub(super) fn user_type_key(&self, path: &syn::Path) -> Option<Arc<str>> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) | Res::Enum(canon) => Some(canon),
            _ => None,
        }
    }

    /// The `conv` operand of a `?`: the frame's error type, interned in
    /// `try_targets`, or `NO_CONV` when the frame returns no user error.
    pub(super) fn try_conv(&mut self) -> u16 {
        let Some(target) = self.cur().ret_error.clone() else {
            return NO_CONV;
        };
        let table = &mut self.cur().try_targets;
        if let Some(index) = table.iter().position(|known| *known == target) {
            return idx16(index);
        }
        table.push(target);
        idx16(table.len() - 1)
    }

    /// Emit a `BuildDefault` for the lowered type.
    pub(super) fn emit_default(&mut self, dst: Reg, ir: DefaultIr) {
        let table = &mut self.cur().defaults;
        table.push(ir);
        let index = idx16(table.len() - 1);
        self.emit(Op::BuildDefault { dst, ir: index });
    }
}
