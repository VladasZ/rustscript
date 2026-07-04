//! Lower the `syn` AST into register bytecode. Runs once per program at load.
//! Every variable is resolved to a register slot here, so the VM never does a
//! name lookup. Control flow becomes jumps, patterns become test-and-bind ops,
//! and the common macros are lowered inline.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use syn::punctuated::Punctuated;
use syn::{BinOp, Block, Expr, FnArg, Lit, Pat, Stmt, UnOp};

use super::bytecode::{
    BinKind, CapSource, Chunk, FmtSpec, MacroKind, Member, Op, PatInfo, Reg, StructLit, UnKind,
};
use super::value::Value;

/// Program level facts the compiler needs, filled before any body is compiled.
pub struct Ctx {
    pub fn_index: HashMap<String, u32>,
    pub structs: HashMap<String, Rc<syn::ItemStruct>>,
}

/// Per function compilation state. A stack of these supports nested closures.
struct FnState {
    code: Vec<Op>,
    consts: Vec<Value>,
    members: Vec<Member>,
    pats: Vec<PatInfo>,
    fmts: Vec<FmtSpec>,
    struct_lits: Vec<StructLit>,
    casts: Vec<Rc<syn::Type>>,
    paths: Vec<(Vec<String>, Option<Rc<syn::Type>>)>,
    names: Vec<String>,
    children: Vec<Rc<Chunk>>,
    child_caps: Vec<Vec<CapSource>>,
    upvalues: Vec<(String, CapSource)>,
    scopes: Vec<HashMap<String, Reg>>,
    reg_top: Reg,
    max_reg: Reg,
    num_params: usize,
    name: String,
}

impl FnState {
    fn new(name: String) -> FnState {
        FnState {
            code: Vec::new(),
            consts: Vec::new(),
            members: Vec::new(),
            pats: Vec::new(),
            fmts: Vec::new(),
            struct_lits: Vec::new(),
            casts: Vec::new(),
            paths: Vec::new(),
            names: Vec::new(),
            children: Vec::new(),
            child_caps: Vec::new(),
            upvalues: Vec::new(),
            scopes: vec![HashMap::new()],
            reg_top: 0,
            max_reg: 0,
            num_params: 0,
            name,
        }
    }

    fn local_reg(&self, name: &str) -> Option<Reg> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn upvalue_index(&self, name: &str) -> Option<u16> {
        self.upvalues.iter().position(|(n, _)| n == name).map(|i| i as u16)
    }

    fn into_chunk(self) -> Chunk {
        Chunk {
            code: self.code,
            num_regs: self.max_reg as usize,
            num_params: self.num_params,
            name: self.name,
            consts: self.consts,
            members: self.members,
            pats: self.pats,
            fmts: self.fmts,
            struct_lits: self.struct_lits,
            casts: self.casts,
            paths: self.paths,
            names: self.names,
            children: self.children,
            child_caps: self.child_caps,
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
    ctx: &'a Ctx,
    frames: Vec<FnState>,
    loops: Vec<LoopCtx>,
}

/// Where a referenced name lives.
enum NameLoc {
    Local(Reg),
    Upvalue(u16),
    /// Not a variable, so a function, enum variant, or other path value.
    None,
}

impl<'a> Compiler<'a> {
    pub fn new(ctx: &'a Ctx) -> Compiler<'a> {
        Compiler { ctx, frames: Vec::new(), loops: Vec::new() }
    }

    /// Compile a top level function or a method body into a chunk.
    pub fn compile_fn(&mut self, sig: &syn::Signature, block: &Block) -> Result<Chunk> {
        self.frames.push(FnState::new(sig.ident.to_string()));
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
        Ok(self.frames.pop().unwrap().into_chunk())
    }

    // -- frame helpers -----------------------------------------------------

    fn cur(&mut self) -> &mut FnState {
        self.frames.last_mut().unwrap()
    }

    fn emit(&mut self, op: Op) {
        self.cur().code.push(op);
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
        self.cur().scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.cur().scopes.pop();
    }

    fn define(&mut self, name: &str, reg: Reg) {
        self.cur().scopes.last_mut().unwrap().insert(name.to_string(), reg);
    }

    fn add_const(&mut self, v: Value) -> u16 {
        let f = self.cur();
        f.consts.push(v);
        (f.consts.len() - 1) as u16
    }

    fn add_member(&mut self, m: Member) -> u16 {
        let f = self.cur();
        f.members.push(m);
        (f.members.len() - 1) as u16
    }

    fn add_cast(&mut self, ty: syn::Type) -> u16 {
        let f = self.cur();
        f.casts.push(Rc::new(ty));
        (f.casts.len() - 1) as u16
    }

    fn add_name(&mut self, name: String) -> u16 {
        let f = self.cur();
        f.names.push(name);
        (f.names.len() - 1) as u16
    }

    fn add_path(&mut self, segs: Vec<String>, coerce: Option<Rc<syn::Type>>) -> u16 {
        let f = self.cur();
        f.paths.push((segs, coerce));
        (f.paths.len() - 1) as u16
    }

    // -- name resolution ---------------------------------------------------

    fn resolve(&mut self, name: &str) -> NameLoc {
        let depth = self.frames.len() - 1;
        if let Some(reg) = self.frames[depth].local_reg(name) {
            return NameLoc::Local(reg);
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
            return Some(self.add_upvalue(depth, name, CapSource::Local(reg)));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            return Some(self.add_upvalue(depth, name, CapSource::Upvalue(idx)));
        }
        let idx = self.capture(parent, name)?;
        Some(self.add_upvalue(depth, name, CapSource::Upvalue(idx)))
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
            NameLoc::Upvalue(idx) => {
                self.emit(Op::LoadUpvalue { dst, idx });
                Ok(())
            }
            NameLoc::None => {
                // A path value: None, a unit enum variant, or a bare constructor.
                let path = self.add_path(vec![name.to_string()], None);
                self.emit(Op::PathValue { dst, path });
                Ok(())
            }
        }
    }

    // -- blocks and statements --------------------------------------------

    fn compile_block(&mut self, block: &Block, dst: Reg) -> Result<()> {
        self.push_scope();
        let res = self.compile_block_inner(block, dst);
        self.pop_scope();
        res
    }

    fn compile_block_inner(&mut self, block: &Block, dst: Reg) -> Result<()> {
        if block.stmts.is_empty() {
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        let last = block.stmts.len() - 1;
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i == last;
            match stmt {
                Stmt::Local(local) => {
                    let val = self.alloc();
                    match &local.init {
                        Some(init) => self.compile_into(val, &init.expr)?,
                        None => self.emit(Op::LoadUnit { dst: val }),
                    }
                    // A type annotation coerces a dynamic value into that type.
                    if let Pat::Type(t) = &local.pat {
                        self.emit_coerce(val, &t.ty);
                        self.bind_pattern_irrefutable(&t.pat, val)?;
                    } else {
                        self.bind_pattern_irrefutable(&local.pat, val)?;
                    }
                    if is_last {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
                Stmt::Expr(expr, semi) => {
                    if is_last && semi.is_none() {
                        self.compile_into(dst, expr)?;
                    } else {
                        let tmp = self.alloc();
                        self.compile_into(tmp, expr)?;
                        if is_last {
                            self.emit(Op::LoadUnit { dst });
                        }
                    }
                }
                Stmt::Item(item) => {
                    if let syn::Item::Fn(_) = item {
                        bail!("unsupported feature: nested functions");
                    }
                    if is_last {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
                Stmt::Macro(m) => {
                    let target = if is_last { dst } else { self.alloc() };
                    self.compile_macro(&m.mac, target)?;
                    if is_last && !macro_yields_value(&m.mac) {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
            }
        }
        Ok(())
    }

    /// Emit a coercion of `reg` into the annotated type when it names a struct,
    /// `Vec<T>`, or `Option<T>`. Falls back to a no-op path the VM understands.
    fn emit_coerce(&mut self, reg: Reg, ty: &syn::Type) {
        let idx = self.add_cast(ty.clone());
        self.emit(Op::Coerce { dst: reg, src: reg, ty: idx });
    }

    // -- expressions -------------------------------------------------------

    /// Compile `expr`, returning the register holding its value. A plain local
    /// returns its own register with no copy.
    fn compile_expr(&mut self, expr: &Expr) -> Result<Reg> {
        if let Expr::Path(p) = expr
            && p.path.segments.len() == 1
            && p.qself.is_none()
        {
            let name = p.path.segments[0].ident.to_string();
            if let NameLoc::Local(reg) = self.resolve(&name) {
                return Ok(reg);
            }
        }
        let dst = self.alloc();
        self.compile_into(dst, expr)?;
        Ok(dst)
    }

    fn compile_into(&mut self, dst: Reg, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Lit(lit) => self.compile_lit(dst, &lit.lit)?,
            Expr::Paren(p) => self.compile_into(dst, &p.expr)?,
            Expr::Group(g) => self.compile_into(dst, &g.expr)?,
            Expr::Reference(r) => self.compile_into(dst, &r.expr)?,
            Expr::Unsafe(u) => self.compile_block(&u.block, dst)?,
            Expr::Block(b) => self.compile_block(&b.block, dst)?,
            Expr::Path(p) => self.compile_path(dst, &p.path)?,
            Expr::Unary(u) => self.compile_unary(dst, u)?,
            Expr::Binary(b) => self.compile_binary(dst, b)?,
            Expr::Assign(a) => {
                self.compile_assign(&a.left, &a.right)?;
                self.emit(Op::LoadUnit { dst });
            }
            Expr::If(if_expr) => self.compile_if(dst, if_expr)?,
            Expr::While(w) => self.compile_while(dst, w)?,
            Expr::ForLoop(f) => self.compile_for(dst, f)?,
            Expr::Loop(l) => self.compile_loop(dst, l)?,
            Expr::Match(m) => self.compile_match(dst, m)?,
            Expr::Return(r) => {
                let src = match &r.expr {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        let u = self.alloc();
                        self.emit(Op::LoadUnit { dst: u });
                        u
                    }
                };
                self.emit(Op::Ret { src });
            }
            Expr::Break(b) => self.compile_break(b)?,
            Expr::Continue(_) => self.compile_continue()?,
            Expr::Call(c) => self.compile_call(dst, c)?,
            Expr::MethodCall(m) => self.compile_method(dst, m)?,
            Expr::Macro(m) => self.compile_macro(&m.mac, dst)?,
            Expr::Tuple(t) => {
                let base = self.compile_args(t.elems.iter())?;
                self.emit(Op::MakeTuple { dst, base, count: t.elems.len() as u16 });
            }
            Expr::Array(a) => {
                let base = self.compile_args(a.elems.iter())?;
                self.emit(Op::MakeVec { dst, base, count: a.elems.len() as u16 });
            }
            Expr::Repeat(r) => {
                let val = self.compile_expr(&r.expr)?;
                let count = self.compile_expr(&r.len)?;
                self.emit(Op::MakeArrayRepeat { dst, val, count });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                self.emit(Op::Index { dst, base, key });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::GetField { dst, base, member });
            }
            Expr::Struct(s) => self.compile_struct_literal(dst, s)?,
            Expr::Range(r) => self.compile_range(dst, r)?,
            Expr::Try(t) => {
                let src = self.compile_expr(&t.expr)?;
                self.emit(Op::Try { dst, src });
            }
            Expr::Cast(c) => {
                let src = self.compile_expr(&c.expr)?;
                let ty = self.add_cast((*c.ty).clone());
                self.emit(Op::Cast { dst, src, ty });
            }
            Expr::Closure(c) => self.compile_closure(dst, c)?,
            Expr::Async(_) | Expr::Await(_) => bail!("unsupported feature: async is not supported"),
            other => bail!("unsupported expression: {}", expr_kind(other)),
        }
        Ok(())
    }

    fn compile_lit(&mut self, dst: Reg, lit: &Lit) -> Result<()> {
        match lit {
            Lit::Int(i) => {
                let v = i.base10_parse::<i64>()?;
                self.emit(Op::LoadInt { dst, v });
            }
            Lit::Bool(b) => self.emit(Op::LoadBool { dst, v: b.value }),
            Lit::Float(f) => {
                let k = self.add_const(Value::Float(f.base10_parse::<f64>()?));
                self.emit(Op::LoadConst { dst, k });
            }
            Lit::Str(s) => {
                let k = self.add_const(Value::str(s.value()));
                self.emit(Op::LoadConst { dst, k });
            }
            Lit::Char(c) => {
                let k = self.add_const(Value::Char(c.value()));
                self.emit(Op::LoadConst { dst, k });
            }
            Lit::Byte(b) => self.emit(Op::LoadInt { dst, v: b.value() as i64 }),
            Lit::ByteStr(bs) => {
                let items = bs.value().into_iter().map(|b| Value::Int(b as i64)).collect();
                let k = self.add_const(Value::vec(items));
                self.emit(Op::LoadConst { dst, k });
            }
            other => bail!("unsupported literal: {other:?}"),
        }
        Ok(())
    }

    fn compile_path(&mut self, dst: Reg, path: &syn::Path) -> Result<()> {
        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            return self.load_name(&name, dst);
        }
        // A multi segment path used as a value, resolved by the VM.
        let segs = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let p = self.add_path(segs, None);
        self.emit(Op::PathValue { dst, path: p });
        Ok(())
    }

    fn compile_unary(&mut self, dst: Reg, u: &syn::ExprUnary) -> Result<()> {
        if matches!(u.op, UnOp::Deref(_)) {
            return self.compile_into(dst, &u.expr);
        }
        let a = self.compile_expr(&u.expr)?;
        let op = match u.op {
            UnOp::Neg(_) => UnKind::Neg,
            UnOp::Not(_) => UnKind::Not,
            _ => bail!("unsupported unary operator"),
        };
        self.emit(Op::Un { dst, a, op });
        Ok(())
    }

    fn compile_binary(&mut self, dst: Reg, b: &syn::ExprBinary) -> Result<()> {
        // Compound assignment, `a += b`, mutates in place and yields unit.
        if is_assign_op(&b.op) {
            let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
            self.compile_compound_assign(&b.left, op, &b.right)?;
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        // Short circuiting logical operators.
        match b.op {
            BinOp::And(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.here() as u32;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            BinOp::Or(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfTrue { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.here() as u32;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            _ => {}
        }
        let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
        let a = self.compile_expr(&b.left)?;
        let c = self.compile_expr(&b.right)?;
        self.emit(Op::Bin { dst, a, b: c, op });
        Ok(())
    }

    fn compile_range(&mut self, dst: Reg, r: &syn::ExprRange) -> Result<()> {
        let start = match &r.start {
            Some(e) => self.compile_expr(e)?,
            None => {
                let z = self.alloc();
                self.emit(Op::LoadInt { dst: z, v: 0 });
                z
            }
        };
        let end = match &r.end {
            Some(e) => self.compile_expr(e)?,
            None => bail!("open ended ranges are not supported"),
        };
        let inclusive = matches!(r.limits, syn::RangeLimits::Closed(_));
        self.emit(Op::MakeRange { dst, start, end, inclusive });
        Ok(())
    }

    // -- control flow ------------------------------------------------------

    fn compile_if(&mut self, dst: Reg, if_expr: &syn::ExprIf) -> Result<()> {
        if let Expr::Let(let_expr) = &*if_expr.cond {
            // `if let PAT = EXPR { .. } else { .. }`.
            let scrut = self.compile_expr(&let_expr.expr)?;
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&let_expr.pat)?;
            self.emit(Op::TestBind { val: scrut, pat, dst: matched });
            let jmp_else = self.here();
            self.emit(Op::JumpIfFalse { cond: matched, to: 0 });
            self.compile_block_inner(&if_expr.then_branch, dst)?;
            self.pop_scope();
            let jmp_end = self.here();
            self.emit(Op::Jump { to: 0 });
            let else_at = self.here() as u32;
            self.patch_jump(jmp_else, else_at);
            match &if_expr.else_branch {
                Some((_, e)) => self.compile_into(dst, e)?,
                None => self.emit(Op::LoadUnit { dst }),
            }
            let end = self.here() as u32;
            self.patch_jump(jmp_end, end);
            return Ok(());
        }
        let cond = self.compile_expr(&if_expr.cond)?;
        let jmp_else = self.here();
        self.emit(Op::JumpIfFalse { cond, to: 0 });
        self.compile_block(&if_expr.then_branch, dst)?;
        let jmp_end = self.here();
        self.emit(Op::Jump { to: 0 });
        let else_at = self.here() as u32;
        self.patch_jump(jmp_else, else_at);
        match &if_expr.else_branch {
            Some((_, e)) => self.compile_into(dst, e)?,
            None => self.emit(Op::LoadUnit { dst }),
        }
        let end = self.here() as u32;
        self.patch_jump(jmp_end, end);
        Ok(())
    }

    fn compile_while(&mut self, dst: Reg, w: &syn::ExprWhile) -> Result<()> {
        let head = self.here();
        // `while let PAT = EXPR` support.
        if let Expr::Let(let_expr) = &*w.cond {
            let scrut = self.compile_expr(&let_expr.expr)?;
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&let_expr.pat)?;
            self.emit(Op::TestBind { val: scrut, pat, dst: matched });
            let exit = self.here();
            self.emit(Op::JumpIfFalse { cond: matched, to: 0 });
            self.loops.push(LoopCtx { breaks: vec![exit], continue_to: head, result: dst });
            let body = self.alloc();
            self.compile_block_inner(&w.body, body)?;
            self.pop_scope();
            self.emit(Op::Jump { to: head as u32 });
            let end = self.here() as u32;
            let lc = self.loops.pop().unwrap();
            for b in lc.breaks {
                self.patch_jump(b, end);
            }
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        let cond = self.compile_expr(&w.cond)?;
        let exit = self.here();
        self.emit(Op::JumpIfFalse { cond, to: 0 });
        self.loops.push(LoopCtx { breaks: vec![exit], continue_to: head, result: dst });
        let body = self.alloc();
        self.compile_block(&w.body, body)?;
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    fn compile_loop(&mut self, dst: Reg, l: &syn::ExprLoop) -> Result<()> {
        self.emit(Op::LoadUnit { dst });
        let head = self.here();
        self.loops.push(LoopCtx { breaks: Vec::new(), continue_to: head, result: dst });
        let body = self.alloc();
        self.compile_block(&l.body, body)?;
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        Ok(())
    }

    fn compile_for(&mut self, dst: Reg, f: &syn::ExprForLoop) -> Result<()> {
        let src = self.compile_expr(&f.expr)?;
        let iter = self.alloc();
        self.emit(Op::IterInit { dst: iter, src });
        let idx = self.alloc();
        self.emit(Op::LoadInt { dst: idx, v: 0 });
        let val = self.alloc();
        let head = self.here();
        let next = self.here();
        self.emit(Op::ForNext { iter, idx, val, to: 0 });
        self.push_scope();
        self.bind_pattern_irrefutable(&f.pat, val)?;
        self.loops.push(LoopCtx { breaks: vec![next], continue_to: head, result: dst });
        let body = self.alloc();
        self.compile_block_inner(&f.body, body)?;
        self.pop_scope();
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    fn compile_break(&mut self, b: &syn::ExprBreak) -> Result<()> {
        let result = self.loops.last().map(|l| l.result);
        if let Some(result) = result {
            match &b.expr {
                Some(e) => self.compile_into(result, e)?,
                None => {}
            }
        } else {
            bail!("break outside a loop");
        }
        let jmp = self.here();
        self.emit(Op::Jump { to: 0 });
        self.loops.last_mut().unwrap().breaks.push(jmp);
        Ok(())
    }

    fn compile_continue(&mut self) -> Result<()> {
        let to = self.loops.last().map(|l| l.continue_to).ok_or_else(|| anyhow!("continue outside a loop"))?;
        self.emit(Op::Jump { to: to as u32 });
        Ok(())
    }

    fn compile_match(&mut self, dst: Reg, m: &syn::ExprMatch) -> Result<()> {
        let scrut = self.compile_expr(&m.expr)?;
        let mut end_jumps = Vec::new();
        for arm in &m.arms {
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&arm.pat)?;
            self.emit(Op::TestBind { val: scrut, pat, dst: matched });
            let skip = self.here();
            self.emit(Op::JumpIfFalse { cond: matched, to: 0 });
            // Guard.
            let mut guard_skip = None;
            if let Some((_, guard)) = &arm.guard {
                let g = self.compile_expr(guard)?;
                let gs = self.here();
                self.emit(Op::JumpIfFalse { cond: g, to: 0 });
                guard_skip = Some(gs);
            }
            self.compile_into(dst, &arm.body)?;
            let je = self.here();
            self.emit(Op::Jump { to: 0 });
            end_jumps.push(je);
            self.pop_scope();
            let next = self.here() as u32;
            self.patch_jump(skip, next);
            if let Some(gs) = guard_skip {
                self.patch_jump(gs, next);
            }
        }
        // No arm matched, a runtime error mirroring the old behavior.
        let p = self.add_path(vec!["::unreachable_match".to_string()], None);
        self.emit(Op::CallPath { dst, path: p, base: dst, argc: 0 });
        let end = self.here() as u32;
        for j in end_jumps {
            self.patch_jump(j, end);
        }
        Ok(())
    }

    // -- calls -------------------------------------------------------------

    /// Compile arguments into a fresh contiguous register window and return its
    /// base. The window is reserved first so an argument's own temporaries,
    /// allocated above it, cannot break the packing.
    fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        for (i, a) in list.iter().enumerate() {
            self.compile_into(base + i as Reg, a)?;
        }
        Ok(base)
    }

    fn compile_call(&mut self, dst: Reg, c: &syn::ExprCall) -> Result<()> {
        let path = match &*c.func {
            Expr::Path(p) => &p.path,
            _ => bail!("cannot call this kind of expression"),
        };
        let coerce = path.segments.last().and_then(first_generic_type).map(|t| Rc::new(t.clone()));
        let argc = c.args.len() as u16;

        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            // A local closure value called directly.
            if let NameLoc::Local(reg) = self.resolve(&name) {
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallValue { dst, callee: reg, base, argc });
                return Ok(());
            }
            // A known top level function, called directly.
            if let Some(&idx) = self.ctx.fn_index.get(&name) {
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallFn { dst, func: idx, base, argc });
                return Ok(());
            }
        }
        // Everything else, resolved by the VM through the bridge dispatch.
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let p = self.add_path(segs, coerce);
        let base = self.compile_args(c.args.iter())?;
        self.emit(Op::CallPath { dst, path: p, base, argc });
        Ok(())
    }

    fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        let recv = self.compile_expr(&m.receiver)?;
        let base = self.compile_args(m.args.iter())?;
        let name = self.add_name(m.method.to_string());
        self.emit(Op::Method { dst, recv, name, base, argc: m.args.len() as u16 });
        Ok(())
    }

    fn compile_closure(&mut self, dst: Reg, c: &syn::ExprClosure) -> Result<()> {
        self.frames.push(FnState::new("<closure>".to_string()));
        let params: Vec<&Pat> = c.inputs.iter().collect();
        self.cur().num_params = params.len();
        for p in &params {
            let reg = self.alloc();
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => self.bind_pattern_irrefutable(p, reg)?,
            }
        }
        let ret = self.alloc();
        self.compile_into(ret, &c.body)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let chunk = Rc::new(child.into_chunk());
        let parent = self.cur();
        let child_idx = parent.children.len() as u16;
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        self.emit(Op::MakeClosure { dst, child: child_idx });
        Ok(())
    }

    // -- assignment --------------------------------------------------------

    fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                match self.resolve(&name) {
                    NameLoc::Local(reg) => self.compile_into(reg, value)?,
                    _ => bail!("assignment to unknown or captured variable `{name}`"),
                }
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let val = self.compile_expr(value)?;
                self.emit(Op::SetIndex { base, key, val });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                let val = self.compile_expr(value)?;
                self.emit(Op::SetField { base, member, val });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                self.compile_assign(&u.expr, value)?;
            }
            Expr::Paren(p) => self.compile_assign(&p.expr, value)?,
            _ => bail!("invalid assignment target"),
        }
        Ok(())
    }

    fn compile_compound_assign(&mut self, target: &Expr, op: BinKind, rhs: &Expr) -> Result<()> {
        // `a op= b` becomes `a = a op b`.
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let reg = match self.resolve(&name) {
                    NameLoc::Local(reg) => reg,
                    _ => bail!("assignment to unknown or captured variable `{name}`"),
                };
                let b = self.compile_expr(rhs)?;
                self.emit(Op::Bin { dst: reg, a: reg, b, op });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let cur = self.alloc();
                self.emit(Op::Index { dst: cur, base, key });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin { dst: res, a: cur, b, op });
                self.emit(Op::SetIndex { base, key, val: res });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                let cur = self.alloc();
                self.emit(Op::GetField { dst: cur, base, member });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin { dst: res, a: cur, b, op });
                self.emit(Op::SetField { base, member, val: res });
            }
            _ => bail!("invalid compound assignment target"),
        }
        Ok(())
    }

    fn member_of(&mut self, member: &syn::Member) -> u16 {
        match member {
            syn::Member::Named(n) => self.add_member(Member::Named(n.to_string())),
            syn::Member::Unnamed(i) => self.add_member(Member::Indexed(i.index as usize)),
        }
    }

    fn compile_struct_literal(&mut self, dst: Reg, s: &syn::ExprStruct) -> Result<()> {
        let name = s.path.segments.last().map(|seg| seg.ident.to_string()).unwrap_or_default();
        // Written fields keyed by name.
        let mut written: Vec<(String, &Expr)> = Vec::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            written.push((key, &f.expr));
        }
        // Field order follows the declaration when the struct is known.
        // Written fields in declaration order, then any extras. A trailing
        // `..rest` fills whatever was not written.
        let order: Vec<String> = match self.ctx.structs.get(&name) {
            Some(def) => {
                let mut ordered: Vec<String> = def
                    .fields
                    .iter()
                    .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
                    .filter(|k| written.iter().any(|(w, _)| w == k))
                    .collect();
                for (k, _) in &written {
                    if !ordered.contains(k) {
                        ordered.push(k.clone());
                    }
                }
                ordered
            }
            None => written.iter().map(|(k, _)| k.clone()).collect(),
        };
        // Reserve a packed window, then fill it, so field temporaries do not
        // break the packing.
        let has_rest = s.rest.is_some();
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + i as Reg;
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => self.compile_into(dstf, e)?,
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.compile_into(base + order.len() as Reg, rest)?;
        }
        let info = {
            let f = self.cur();
            f.struct_lits.push(StructLit { name, fields: order, has_rest });
            (f.struct_lits.len() - 1) as u16
        };
        self.emit(Op::MakeStruct { dst, info, base });
        Ok(())
    }

    // -- patterns ----------------------------------------------------------

    /// Register a pattern and the slot each bound name uses.
    fn pattern_info(&mut self, pat: &Pat) -> Result<u16> {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        let mut binds = Vec::new();
        for n in names {
            let reg = self.alloc();
            self.define(&n, reg);
            binds.push((n, reg));
        }
        let f = self.cur();
        f.pats.push(PatInfo { pat: Rc::new(pat.clone()), binds });
        Ok((f.pats.len() - 1) as u16)
    }

    /// Bind an irrefutable pattern whose value already sits in `reg`.
    fn bind_pattern_irrefutable(&mut self, pat: &Pat, reg: Reg) -> Result<()> {
        match pat {
            Pat::Ident(id) if id.subpat.is_none() => {
                self.define(&id.ident.to_string(), reg);
                Ok(())
            }
            Pat::Wild(_) => Ok(()),
            Pat::Type(t) => self.bind_pattern_irrefutable(&t.pat, reg),
            Pat::Paren(p) => self.bind_pattern_irrefutable(&p.pat, reg),
            Pat::Reference(r) => self.bind_pattern_irrefutable(&r.pat, reg),
            _ => {
                // Tuple or struct destructuring, use a test-and-bind that always
                // matches for these irrefutable shapes.
                let matched = self.alloc();
                let pidx = self.pattern_info(pat)?;
                self.emit(Op::TestBind { val: reg, pat: pidx, dst: matched });
                Ok(())
            }
        }
    }

    // -- macros ------------------------------------------------------------

    fn compile_macro(&mut self, mac: &syn::Macro, dst: Reg) -> Result<()> {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        match name.as_str() {
            "println" | "print" | "eprintln" | "eprint" | "panic" | "anyhow" | "bail" => {
                let spec = self.build_fmt_spec(mac)?;
                let kind = match name.as_str() {
                    "println" => MacroKind::Println,
                    "print" => MacroKind::Print,
                    "eprintln" => MacroKind::Eprintln,
                    "eprint" => MacroKind::Eprint,
                    "panic" => MacroKind::Panic,
                    "anyhow" => MacroKind::Anyhow,
                    _ => MacroKind::Bail,
                };
                self.emit(Op::MacroCall { kind, dst, spec });
            }
            "format" => {
                let spec = self.build_fmt_spec(mac)?;
                self.emit(Op::Fmt { dst, spec });
            }
            "vec" => self.compile_vec_macro(dst, mac)?,
            "assert" => {
                let args = parse_exprs(mac)?;
                let cond = args.first().ok_or_else(|| anyhow!("assert! needs a condition"))?;
                let c = self.compile_expr(cond)?;
                let ok = self.here();
                self.emit(Op::JumpIfTrue { cond: c, to: 0 });
                let p = self.add_path(vec!["::assert_failed".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: dst, argc: 0 });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "assert_eq" | "assert_ne" => {
                let args = parse_exprs(mac)?;
                let a = self.compile_expr(args.first().ok_or_else(|| anyhow!("assert needs two args"))?)?;
                let b = self.compile_expr(args.get(1).ok_or_else(|| anyhow!("assert needs two args"))?)?;
                let eqr = self.alloc();
                self.emit(Op::Bin { dst: eqr, a, b, op: BinKind::Eq });
                let ok = self.here();
                if name == "assert_eq" {
                    self.emit(Op::JumpIfTrue { cond: eqr, to: 0 });
                } else {
                    self.emit(Op::JumpIfFalse { cond: eqr, to: 0 });
                }
                let p = self.add_path(vec!["::assert_failed".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: dst, argc: 0 });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "matches" => {
                let (expr, pat, guard) = parse_matches(mac)?;
                let scrut = self.compile_expr(&expr)?;
                self.push_scope();
                let pidx = self.pattern_info(&pat)?;
                self.emit(Op::TestBind { val: scrut, pat: pidx, dst });
                if let Some(g) = guard {
                    let skip = self.here();
                    self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
                    self.compile_into(dst, &g)?;
                    let end = self.here() as u32;
                    self.patch_jump(skip, end);
                }
                self.pop_scope();
            }
            "ensure" => {
                let args = parse_exprs(mac)?;
                let cond = args.first().ok_or_else(|| anyhow!("ensure! needs a condition"))?;
                let c = self.compile_expr(cond)?;
                let ok = self.here();
                self.emit(Op::JumpIfTrue { cond: c, to: 0 });
                // Build the error message and return it.
                let msg = self.alloc();
                if let Some(m) = args.get(1) {
                    self.compile_into(msg, m)?;
                } else {
                    let k = self.add_const(Value::str("condition failed"));
                    self.emit(Op::LoadConst { dst: msg, k });
                }
                let p = self.add_path(vec!["::ensure_fail".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: msg, argc: 1 });
                self.emit(Op::Ret { src: dst });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "dbg" => {
                let args = parse_exprs(mac)?;
                let base = self.compile_args(args.iter())?;
                self.emit(Op::Dbg { dst, base, argc: args.len() as u16 });
            }
            other => bail!("unsupported macro: {other}!"),
        }
        Ok(())
    }

    fn compile_vec_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        if let Ok(rep) = mac.parse_body_with(parse_vec_repeat) {
            let val = self.compile_expr(&rep.0)?;
            let count = self.compile_expr(&rep.1)?;
            self.emit(Op::MakeArrayRepeat { dst, val, count });
            return Ok(());
        }
        let exprs = parse_exprs(mac)?;
        let base = self.compile_args(exprs.iter())?;
        self.emit(Op::MakeVec { dst, base, count: exprs.len() as u16 });
        Ok(())
    }

    /// Parse a format macro body and compile its arguments, resolving inline
    /// `{name}` holes to variables in scope.
    fn build_fmt_spec(&mut self, mac: &syn::Macro) -> Result<u16> {
        let args = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
        let mut iter = args.iter();
        let template = match iter.next() {
            Some(Expr::Lit(l)) => match &l.lit {
                Lit::Str(s) => s.value(),
                _ => bail!("format template must be a string literal"),
            },
            Some(_) => bail!("format template must be a string literal"),
            None => String::new(),
        };
        let mut positional = Vec::new();
        let mut named: Vec<(String, Reg)> = Vec::new();
        for arg in iter {
            if let Expr::Assign(a) = arg
                && let Expr::Path(p) = &*a.left
                && let Some(n) = p.path.get_ident()
            {
                let r = self.compile_expr(&a.right)?;
                named.push((n.to_string(), r));
                continue;
            }
            let r = self.compile_expr(arg)?;
            positional.push(r);
        }
        // Inline identifiers referenced in the template but not given explicitly.
        for hole in inline_holes(&template) {
            if named.iter().all(|(n, _)| n != &hole) {
                let r = self.alloc();
                self.load_name(&hole, r)?;
                named.push((hole, r));
            }
        }
        let f = self.cur();
        f.fmts.push(FmtSpec { template, positional, named });
        Ok((f.fmts.len() - 1) as u16)
    }

    // -- jump patching -----------------------------------------------------

    fn patch_jump(&mut self, at: usize, to: u32) {
        match &mut self.cur().code[at] {
            Op::Jump { to: t }
            | Op::JumpIfFalse { to: t, .. }
            | Op::JumpIfTrue { to: t, .. }
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
        Pat::Ident(id) => {
            out.push(id.ident.to_string());
            if let Some(sub) = &id.subpat {
                collect_pattern_names(&sub.1, out);
            }
        }
        Pat::Tuple(t) => t.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::TupleStruct(ts) => ts.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Slice(s) => s.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Struct(s) => s.fields.iter().for_each(|f| collect_pattern_names(&f.pat, out)),
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
            let arg = inner.split(':').next().unwrap_or("").trim();
            if !arg.is_empty()
                && arg.parse::<usize>().is_err()
                && arg.chars().all(|c| c.is_alphanumeric() || c == '_')
                && arg.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                out.push(arg.to_string());
            }
        } else if c == '}' && chars.peek() == Some(&'}') {
            chars.next();
        }
    }
    out
}

fn macro_yields_value(mac: &syn::Macro) -> bool {
    let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
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
