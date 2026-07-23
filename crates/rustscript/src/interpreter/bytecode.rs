//! The compiled form a script runs as. The compiler lowers the `syn` AST into a
//! `Chunk` of register based instructions once, then the VM executes it without
//! ever touching the parse tree again. Registers are numbered slots in a flat
//! frame, so variable access is an array read, not a name lookup.

use std::rc::Rc;
use std::sync::Arc;

pub type Reg = u16;

/// A literal constant baked into a chunk, value model neutral so both the fast
/// `Rc` engine and the parallel `Arc` engine share the same compiler output.
/// Each engine materializes a `Const` into its own value type when a
/// `LoadConst` runs.
/// Only literals that need a side table land here. Integers, booleans, and unit
/// are emitted as their own inline load ops, so they are not `Const` variants.
#[derive(Clone)]
pub enum Const {
    Float(f64),
    Char(char),
    Str(Arc<str>),
    /// A byte string literal `b"..."`, materialized into a vec of integers.
    Bytes(Arc<[u8]>),
}

/// Sentinel destination for a method call whose result a statement discards.
/// The VM skips building and writing the return value, which lets hot ops
/// like map insert avoid allocating a `Some(old)` nobody reads.
pub const DISCARD: Reg = Reg::MAX;

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
    Named(Rc<str>),
    Indexed(usize),
}

/// Where a closure upvalue is copied from when the closure is built.
#[derive(Clone, Copy)]
pub enum CapSource {
    /// A local register in the enclosing function.
    Local(Reg),
    /// An upvalue of the enclosing closure.
    Upvalue(u16),
    /// A local register shared through a mutable capture cell.
    MutableLocal(Reg),
    /// A mutable capture cell from the enclosing closure.
    MutableUpvalue(u16),
}

impl CapSource {
    pub fn is_mutable(self) -> bool {
        matches!(self, Self::MutableLocal(_) | Self::MutableUpvalue(_))
    }
}

/// A struct literal, fields already ordered to match the declaration so
/// serialization matches the compiler. The shape is built once at compile
/// time and shared by every instance the literal creates.
pub struct StructLit {
    /// Field names in the register order `base..base+fields.len()`.
    pub shape: Rc<super::value::StructShape>,
    /// Whether a trailing `..rest` value sits in the register after the fields.
    pub has_rest: bool,
}

#[derive(Clone)]
pub struct EnumVariant {
    pub enum_name: Rc<str>,
    pub variant: Rc<str>,
}

/// A method name with its builtin id resolved once at compile time, so hot
/// dispatch matches an enum instead of comparing strings.
#[derive(Clone)]
pub struct MethodName {
    pub text: String,
    pub id: BuiltinId,
}

/// Ids for the builtin and higher-order methods the dispatcher special-cases.
/// `Other` falls back to name-string dispatch, so an unlisted method still
/// works, it just pays the string compares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinId {
    Len,
    IsEmpty,
    Clone,
    ToString,
    Get,
    Insert,
    ContainsKey,
    Remove,
    Entry,
    Keys,
    Values,
    Iter,
    IterMut,
    Push,
    Pop,
    First,
    Last,
    Contains,
    Sort,
    Join,
    Concat,
    Sum,
    Enumerate,
    Rev,
    Count,
    Take,
    Skip,
    PushStr,
    /// `bool::then`, which takes a closure.
    Then,
    /// `clone_from`, which replaces the receiver and so is handled by the VM.
    CloneFrom,
    SplitWhitespace,
    Split,
    Chars,
    Lines,
    Trim,
    StartsWith,
    EndsWith,
    Parse,
    Unwrap,
    UnwrapOr,
    Copied,
    // Higher-order methods, dispatched before the plain builtins.
    Map,
    Filter,
    FilterMap,
    FlatMap,
    ForEach,
    Find,
    Position,
    Any,
    All,
    Fold,
    Reduce,
    Retain,
    SortByKey,
    SortByCachedKey,
    SortBy,
    MaxByKey,
    MinByKey,
    TakeWhile,
    SkipWhile,
    Partition,
    AndThen,
    MapErr,
    MapOr,
    UnwrapOrElse,
    OkOrElse,
    WithContext,
    OrInsertWith,
    OrInsertWithKey,
    AndModify,
    Other,
}

impl BuiltinId {
    pub fn resolve(name: &str) -> BuiltinId {
        use BuiltinId::*;
        match name {
            "len" => Len,
            "is_empty" => IsEmpty,
            "clone" => Clone,
            "to_string" => ToString,
            // A returned container is Rc shared, so mutating it reaches the
            // original. That is what `get_mut` is for, so it resolves to the
            // same op rather than needing a mutable borrow the VM has no
            // concept of.
            "get" | "get_mut" => Get,
            "then" => Then,
            "clone_from" => CloneFrom,
            "insert" => Insert,
            "contains_key" => ContainsKey,
            "remove" => Remove,
            "entry" => Entry,
            "keys" => Keys,
            "values" => Values,
            "iter" | "into_iter" => Iter,
            "iter_mut" => IterMut,
            "push" => Push,
            "pop" => Pop,
            "first" => First,
            "last" => Last,
            "contains" => Contains,
            "sort" => Sort,
            "join" => Join,
            "concat" => Concat,
            "sum" => Sum,
            "enumerate" => Enumerate,
            "rev" => Rev,
            "count" => Count,
            "take" => Take,
            "skip" => Skip,
            "push_str" => PushStr,
            "split_whitespace" => SplitWhitespace,
            "split" => Split,
            "chars" => Chars,
            "lines" => Lines,
            "trim" => Trim,
            "starts_with" => StartsWith,
            "ends_with" => EndsWith,
            "parse" => Parse,
            "unwrap" => Unwrap,
            "unwrap_or" => UnwrapOr,
            "copied" | "cloned" => Copied,
            "map" => Map,
            "filter" => Filter,
            "filter_map" => FilterMap,
            "flat_map" => FlatMap,
            "for_each" => ForEach,
            "find" => Find,
            "position" => Position,
            "any" => Any,
            "all" => All,
            "fold" => Fold,
            "reduce" => Reduce,
            "retain" => Retain,
            "sort_by_key" => SortByKey,
            "sort_by_cached_key" => SortByCachedKey,
            "sort_by" => SortBy,
            "max_by_key" => MaxByKey,
            "min_by_key" => MinByKey,
            "take_while" => TakeWhile,
            "skip_while" => SkipWhile,
            "partition" => Partition,
            "and_then" => AndThen,
            "map_err" => MapErr,
            "map_or" => MapOr,
            "unwrap_or_else" => UnwrapOrElse,
            "ok_or_else" => OkOrElse,
            "with_context" => WithContext,
            "or_insert_with" => OrInsertWith,
            "or_insert_with_key" => OrInsertWithKey,
            "and_modify" => AndModify,
            _ => Other,
        }
    }

    /// Whether this method takes a closure and must run through the
    /// interpreter's higher-order dispatch.
    pub fn is_higher_order(self) -> bool {
        use BuiltinId::*;
        matches!(
            self,
            Then | Map
                | Filter
                | FilterMap
                | FlatMap
                | ForEach
                | Find
                | Position
                | Any
                | All
                | Fold
                | Reduce
                | Retain
                | SortByKey
                | SortByCachedKey
                | SortBy
                | MaxByKey
                | MinByKey
                | TakeWhile
                | SkipWhile
                | Partition
                | AndThen
                | MapErr
                | MapOr
                | UnwrapOrElse
                | OkOrElse
                | WithContext
                | OrInsertWith
                | OrInsertWithKey
                | AndModify
                | Other
        )
    }
}

/// A precompiled format template plus where each argument lives.
#[derive(Clone)]
pub struct FmtSpec {
    pub template: String,
    /// Positional argument registers, in order.
    pub positional: Vec<Reg>,
    /// Named and inline `{name}` arguments.
    pub named: Vec<(String, Reg)>,
}

/// A pattern plus the register each name it binds writes into.
pub struct PatInfo {
    pub pat: PPat,
    pub binds: Vec<(String, Reg)>,
}

#[derive(Clone)]
pub enum PLit {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char),
}

#[derive(Clone)]
pub enum PPat {
    Wild,
    Rest,
    Ident {
        name: String,
        sub: Option<Box<PPat>>,
    },
    Lit(PLit),
    Tuple(Vec<PPat>),
    TupleStruct {
        name: Option<String>,
        elems: Vec<PPat>,
    },
    Path {
        name: Option<String>,
    },
    Struct {
        name: Option<String>,
        fields: Vec<(String, PPat)>,
    },
    Or(Vec<PPat>),
    Slice(Vec<PPat>),
    /// A literal range like `b'a'..=b'z'`, `'0'..='9'`, or `1..5`. A missing
    /// endpoint leaves that side unbounded.
    Range {
        lo: Option<PLit>,
        hi: Option<PLit>,
        inclusive: bool,
    },
    Unsupported,
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

#[derive(Clone)]
pub enum Op {
    LoadConst {
        dst: Reg,
        k: u16,
    },
    LoadInt {
        dst: Reg,
        v: i64,
    },
    LoadBool {
        dst: Reg,
        v: bool,
    },
    LoadUnit {
        dst: Reg,
    },
    LoadUpvalue {
        dst: Reg,
        idx: u16,
    },
    LoadCell {
        dst: Reg,
        cell: Reg,
    },
    StoreCell {
        cell: Reg,
        src: Reg,
    },
    StoreUpvalue {
        idx: u16,
        src: Reg,
    },
    /// Read a module level const or static, evaluated lazily on first use.
    LoadGlobal {
        dst: Reg,
        idx: u32,
    },
    Move {
        dst: Reg,
        src: Reg,
    },

    Bin {
        dst: Reg,
        a: Reg,
        b: Reg,
        op: BinKind,
    },
    /// Binary op with an integer literal right operand, `n - 1`, `i < len`.
    BinImm {
        dst: Reg,
        a: Reg,
        imm: i64,
        op: BinKind,
    },
    Un {
        dst: Reg,
        a: Reg,
        op: UnKind,
    },

    Jump {
        to: u32,
    },
    JumpIfFalse {
        cond: Reg,
        to: u32,
    },
    JumpIfTrue {
        cond: Reg,
        to: u32,
    },
    /// Fused compare and branch: jump to `to` when `a op b` is false.
    CmpJump {
        a: Reg,
        b: Reg,
        op: BinKind,
        to: u32,
    },
    /// Fused compare and branch against an integer literal.
    CmpJumpImm {
        a: Reg,
        imm: i64,
        op: BinKind,
        to: u32,
    },

    /// Direct call of a known top level function, by global index.
    /// `targ` indexes the caller chunk's `call_type_args`, or `u32::MAX` when
    /// the call had no turbofish type arguments.
    CallFn {
        dst: Reg,
        func: u32,
        base: Reg,
        argc: u16,
        targ: u32,
    },
    /// Call a closure value held in a register.
    CallValue {
        dst: Reg,
        callee: Reg,
        base: Reg,
        argc: u16,
    },
    /// Any other call, `Type::assoc`, a bridge, a constructor, resolved by path.
    CallPath {
        dst: Reg,
        path: u16,
        base: Reg,
        argc: u16,
    },
    /// A path used as a value, `None`, a unit enum variant, `consts::OS`.
    PathValue {
        dst: Reg,
        path: u16,
    },
    /// `recv.method(args)`.
    Method {
        dst: Reg,
        recv: Reg,
        name: u16,
        base: Reg,
        argc: u16,
    },
    /// Fused `recv.get(key).copied().unwrap_or(default)`. One probe, no
    /// intermediate Option built. Falls back to the three real methods for
    /// receivers that are not a map or a vector.
    GetOrDefault {
        dst: Reg,
        recv: Reg,
        key: Reg,
        default: Reg,
    },
    Ret {
        src: Reg,
    },

    MakeVec {
        dst: Reg,
        base: Reg,
        count: u16,
    },
    MakeTuple {
        dst: Reg,
        base: Reg,
        count: u16,
    },
    MakeArrayRepeat {
        dst: Reg,
        val: Reg,
        count: Reg,
    },
    MakeRange {
        dst: Reg,
        start: Reg,
        end: Reg,
        inclusive: bool,
    },
    /// Materialize any iterable in `src` into an iterator held in `dst`.
    IterInit {
        dst: Reg,
        src: Reg,
    },
    /// Read the next item of the iterator in `iter` into `val`, advancing `idx`.
    /// Jumps to `to` when exhausted.
    ForNext {
        iter: Reg,
        idx: Reg,
        val: Reg,
        to: u32,
    },
    MakeStruct {
        dst: Reg,
        info: u16,
        base: Reg,
    },
    MakeEnum {
        dst: Reg,
        info: u16,
        base: Reg,
        count: u16,
    },
    LoadEnum {
        dst: Reg,
        info: u16,
    },
    MakeClosure {
        dst: Reg,
        child: u16,
    },

    Index {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    SetIndex {
        base: Reg,
        key: Reg,
        val: Reg,
    },
    Deref {
        dst: Reg,
        src: Reg,
    },
    SetDeref {
        target: Reg,
        val: Reg,
    },
    GetField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    SetField {
        base: Reg,
        member: u16,
        val: Reg,
    },

    /// The `?` operator. Unwraps Ok/Some into `dst`, or returns early on Err/None.
    Try {
        dst: Reg,
        src: Reg,
    },
    Cast {
        dst: Reg,
        src: Reg,
        ty: u16,
    },
    /// Coerce a dynamic value into an annotated type, `let c: Config = ..`.
    Coerce {
        dst: Reg,
        src: Reg,
        ty: u16,
    },

    /// Test `val` against a pattern, binding its names into their registers.
    /// `dst` receives a bool.
    TestBind {
        val: Reg,
        pat: u16,
        dst: Reg,
    },

    /// Render a format template into `dst`.
    Fmt {
        dst: Reg,
        spec: u16,
    },
    /// A statement macro that renders a template then acts on it.
    MacroCall {
        kind: MacroKind,
        dst: Reg,
        spec: u16,
    },
    /// `dbg!` takes plain registers, not a template.
    Dbg {
        dst: Reg,
        base: Reg,
        argc: u16,
    },

    /// Spawn child closure `child` as a tokio task, writing a JoinHandle into
    /// `dst`. Emitted only for `#[tokio::main]` scripts, run by the parallel VM.
    Spawn {
        dst: Reg,
        child: u16,
    },
    /// Await the future or JoinHandle in `src`, writing its result into `dst`.
    /// Parallel VM only.
    Await {
        dst: Reg,
        src: Reg,
    },
}

/// One compiled function, method, or closure body.
pub struct Chunk {
    pub code: Vec<Op>,
    /// Source line of each op, parallel to `code`. Zero means unknown, so a
    /// synthesized chunk with no lines still traces by function name alone.
    pub lines: Vec<u32>,
    /// Source file this body was written in, shown in runtime error traces.
    pub file: Arc<str>,
    pub num_regs: usize,
    pub num_params: usize,
    pub name: String,
    /// Module this body was written in, for runtime type resolution.
    pub module: u16,

    // Side tables referenced by instruction operands.
    pub consts: Vec<Const>,
    pub members: Vec<Member>,
    pub pats: Vec<PatInfo>,
    pub fmts: Vec<FmtSpec>,
    pub struct_lits: Vec<StructLit>,
    pub enum_variants: Vec<EnumVariant>,
    pub casts: Vec<Rc<syn::Type>>,
    /// Path calls, the segments plus an optional turbofish coercion type.
    pub paths: Vec<(Vec<String>, Option<Rc<syn::Type>>)>,
    pub names: Vec<MethodName>,
    /// Nested closure bodies, referenced by `MakeClosure`.
    pub children: Vec<Rc<Chunk>>,
    /// For each child, where to copy its upvalues from.
    pub child_caps: Vec<Vec<CapSource>>,
    /// Generic parameter names of this function, in order, e.g. `["T"]`. Used
    /// to bind a caller's turbofish type args when the body resolves them.
    pub generics: Vec<Rc<str>>,
    /// Turbofish type args recorded at `CallFn` sites, referenced by `targ`.
    pub call_type_args: Vec<Rc<[Rc<syn::Type>]>>,
}

impl Chunk {
    pub fn empty(name: impl Into<String>) -> Chunk {
        Chunk {
            code: Vec::new(),
            lines: Vec::new(),
            file: Arc::from(""),
            num_regs: 0,
            num_params: 0,
            name: name.into(),
            module: 0,
            consts: Vec::new(),
            members: Vec::new(),
            pats: Vec::new(),
            fmts: Vec::new(),
            struct_lits: Vec::new(),
            enum_variants: Vec::new(),
            casts: Vec::new(),
            paths: Vec::new(),
            names: Vec::new(),
            children: Vec::new(),
            child_caps: Vec::new(),
            generics: Vec::new(),
            call_type_args: Vec::new(),
        }
    }
}
