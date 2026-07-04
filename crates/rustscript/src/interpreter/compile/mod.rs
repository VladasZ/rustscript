//! Lower the `syn` AST into register bytecode. Runs once per program at load.
//! Every variable is resolved to a register slot here, so the VM never does a
//! name lookup. Control flow becomes jumps, patterns become test-and-bind ops,
//! and the common macros are lowered inline.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use syn::punctuated::Punctuated;
use syn::{BinOp, Block, Expr, FnArg, Lit, Pat, UnOp};

use super::bytecode::{
    BinKind, BuiltinId, CapSource, Chunk, FmtSpec, Member, MethodName, Op, PatInfo, Reg,
    StructLit,
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
    names: Vec<MethodName>,
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
            scopes: vec![HashMap::default()],
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
    /// A `let x: T = from_str(..)...` annotation waiting to attach to that
    /// exact `from_str` call, keyed by the call's address so a nested call
    /// inside its arguments cannot steal it. Lets the typed json path run
    /// without a turbofish.
    pub(super) json_let: Option<(*const syn::ExprCall, Rc<syn::Type>)>,
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
        Compiler { ctx, frames: Vec::new(), loops: Vec::new(), json_let: None }
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
        self.cur().scopes.push(HashMap::default());
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
        f.names.push(MethodName { id: BuiltinId::resolve(&name), text: name });
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

mod calls;
mod expr;
mod macros;
