//! The compiled form a script runs as. The compiler lowers the `syn` AST into a
//! `Chunk` of register based instructions once, then the VM executes it without
//! ever touching the parse tree again. Registers are numbered slots in a flat
//! frame, so variable access is an array read, not a name lookup.

use std::rc::Rc;

use super::value::Value;

pub type Reg = u16;

/// Binary operators, kept separate from `syn` so the hot loop carries no parse
/// tree types.
#[derive(Clone, Copy, Debug)]
pub enum BinKind {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, Debug)]
pub enum UnKind {
    Neg,
    Not,
}

/// A field being read or written, named for structs, positional for tuples.
#[derive(Clone)]
pub enum Member {
    Named(String),
    Indexed(usize),
}

/// Where a closure upvalue is copied from when the closure is built.
#[derive(Clone, Copy)]
pub enum CapSource {
    /// A local register in the enclosing function.
    Local(Reg),
    /// An upvalue of the enclosing closure.
    Upvalue(u16),
}

/// A struct literal, fields already ordered to match the declaration so
/// serialization matches the compiler.
pub struct StructLit {
    pub name: String,
    /// Field names in the register order `base..base+fields.len()`.
    pub fields: Vec<String>,
    /// Whether a trailing `..rest` value sits in the register after the fields.
    pub has_rest: bool,
}

/// A precompiled format template plus where each argument lives.
pub struct FmtSpec {
    pub template: String,
    /// Positional argument registers, in order.
    pub positional: Vec<Reg>,
    /// Named and inline `{name}` arguments.
    pub named: Vec<(String, Reg)>,
}

/// A pattern plus the register each name it binds writes into.
pub struct PatInfo {
    pub pat: Rc<syn::Pat>,
    pub binds: Vec<(String, Reg)>,
}

#[derive(Clone, Copy)]
pub enum MacroKind {
    Println,
    Print,
    Eprintln,
    Eprint,
    Panic,
    Anyhow,
    Bail,
}

pub enum Op {
    LoadConst { dst: Reg, k: u16 },
    LoadInt { dst: Reg, v: i64 },
    LoadBool { dst: Reg, v: bool },
    LoadUnit { dst: Reg },
    LoadUpvalue { dst: Reg, idx: u16 },
    Move { dst: Reg, src: Reg },

    Bin { dst: Reg, a: Reg, b: Reg, op: BinKind },
    Un { dst: Reg, a: Reg, op: UnKind },

    Jump { to: u32 },
    JumpIfFalse { cond: Reg, to: u32 },
    JumpIfTrue { cond: Reg, to: u32 },

    /// Direct call of a known top level function, by global index.
    CallFn { dst: Reg, func: u32, base: Reg, argc: u16 },
    /// Call a closure value held in a register.
    CallValue { dst: Reg, callee: Reg, base: Reg, argc: u16 },
    /// Any other call, `Type::assoc`, a bridge, a constructor, resolved by path.
    CallPath { dst: Reg, path: u16, base: Reg, argc: u16 },
    /// A path used as a value, `None`, a unit enum variant, `consts::OS`.
    PathValue { dst: Reg, path: u16 },
    /// `recv.method(args)`.
    Method { dst: Reg, recv: Reg, name: u16, base: Reg, argc: u16 },
    Ret { src: Reg },

    MakeVec { dst: Reg, base: Reg, count: u16 },
    MakeTuple { dst: Reg, base: Reg, count: u16 },
    MakeArrayRepeat { dst: Reg, val: Reg, count: Reg },
    MakeRange { dst: Reg, start: Reg, end: Reg, inclusive: bool },
    /// Materialize any iterable in `src` into an iterator held in `dst`.
    IterInit { dst: Reg, src: Reg },
    /// Read the next item of the iterator in `iter` into `val`, advancing `idx`.
    /// Jumps to `to` when exhausted.
    ForNext { iter: Reg, idx: Reg, val: Reg, to: u32 },
    MakeStruct { dst: Reg, info: u16, base: Reg },
    MakeClosure { dst: Reg, child: u16 },

    Index { dst: Reg, base: Reg, key: Reg },
    SetIndex { base: Reg, key: Reg, val: Reg },
    GetField { dst: Reg, base: Reg, member: u16 },
    SetField { base: Reg, member: u16, val: Reg },

    /// The `?` operator. Unwraps Ok/Some into `dst`, or returns early on Err/None.
    Try { dst: Reg, src: Reg },
    Cast { dst: Reg, src: Reg, ty: u16 },
    /// Coerce a dynamic value into an annotated type, `let c: Config = ..`.
    Coerce { dst: Reg, src: Reg, ty: u16 },

    /// Test `val` against a pattern, binding its names into their registers.
    /// `dst` receives a bool.
    TestBind { val: Reg, pat: u16, dst: Reg },

    /// Render a format template into `dst`.
    Fmt { dst: Reg, spec: u16 },
    /// A statement macro that renders a template then acts on it.
    MacroCall { kind: MacroKind, dst: Reg, spec: u16 },
    /// `dbg!` takes plain registers, not a template.
    Dbg { dst: Reg, base: Reg, argc: u16 },
}

/// One compiled function, method, or closure body.
pub struct Chunk {
    pub code: Vec<Op>,
    pub num_regs: usize,
    pub num_params: usize,
    pub name: String,

    // Side tables referenced by instruction operands.
    pub consts: Vec<Value>,
    pub members: Vec<Member>,
    pub pats: Vec<PatInfo>,
    pub fmts: Vec<FmtSpec>,
    pub struct_lits: Vec<StructLit>,
    pub casts: Vec<Rc<syn::Type>>,
    /// Path calls, the segments plus an optional turbofish coercion type.
    pub paths: Vec<(Vec<String>, Option<Rc<syn::Type>>)>,
    pub names: Vec<String>,
    /// Nested closure bodies, referenced by `MakeClosure`.
    pub children: Vec<Rc<Chunk>>,
    /// For each child, where to copy its upvalues from.
    pub child_caps: Vec<Vec<CapSource>>,
}

impl Chunk {
    pub fn empty(name: impl Into<String>) -> Chunk {
        Chunk {
            code: Vec::new(),
            num_regs: 0,
            num_params: 0,
            name: name.into(),
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
        }
    }
}
