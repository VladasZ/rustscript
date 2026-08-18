//! The compiled form a script runs as. The compiler lowers the `syn` AST into a
//! `Chunk` of register based instructions once, then the VM executes it without
//! ever touching the parse tree again. Registers are numbered slots in a flat
//! frame, so variable access is an array read, not a name lookup.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use parking_lot::Mutex;

use super::typeir::{CastIr, TypeIr};

pub type Reg = u16;

/// A literal constant baked into a chunk, materialized into a value when a
/// `LoadConst` runs.
/// Only literals that need a side table land here. Integers, booleans, and unit
/// are emitted as their own inline load ops, so they are not `Const` variants.
#[derive(Clone)]
pub enum Const {
    /// A 128-bit integer literal, i128 exact or u128 as reinterpreted bits.
    Big(i128, super::numeric::IntWidth),
    Float(f64),
    /// An f32 literal, parsed from its digits at f32 precision so the value
    /// never takes a detour through f64 rounding.
    F32(f32),
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

/// The verb real Rust uses in the overflow panic for each arithmetic op.
pub fn overflow_message(op: BinKind) -> &'static str {
    match op {
        BinKind::Add => "attempt to add with overflow",
        BinKind::Sub => "attempt to subtract with overflow",
        BinKind::Mul => "attempt to multiply with overflow",
        BinKind::Div => "attempt to divide with overflow",
        BinKind::Rem => "attempt to calculate the remainder with overflow",
        _ => "attempt to compute with overflow",
    }
}

#[derive(Clone, Copy, Debug)]
pub enum UnKind {
    Neg,
    Not,
}

/// Method names whose builtin receivers mutate in place. The compiler
/// splits such a receiver from value sharing before the call, and the VM
/// applies the same split when the receiver arrives through a reference.
/// Read-only names must stay off this list, splitting is correct but wastes
/// a copy when the receiver is shared.
pub fn builtin_mutating(name: &str) -> bool {
    matches!(
        name,
        "push"
            | "pop"
            | "insert"
            | "insert_str"
            | "remove"
            | "swap_remove"
            | "remove_entry"
            | "push_str"
            | "clear"
            | "extend"
            | "extend_from_slice"
            | "append"
            | "truncate"
            | "retain"
            | "retain_mut"
            | "dedup"
            | "dedup_by"
            | "dedup_by_key"
            | "sort"
            | "sort_by"
            | "sort_by_key"
            | "sort_unstable"
            | "sort_unstable_by"
            | "sort_unstable_by_key"
            | "reverse"
            | "rotate_left"
            | "rotate_right"
            | "fill"
            | "resize"
            | "swap"
            | "split_off"
            | "drain"
            | "iter_mut"
            | "values_mut"
            | "get_mut"
            | "first_mut"
            | "last_mut"
            | "entry"
            | "take"
            | "replace"
            | "get_or_insert"
            | "get_or_insert_with"
            | "make_ascii_uppercase"
            | "make_ascii_lowercase"
            | "clone_from"
            | "copy_from_slice"
            | "shuffle"
            | "read_line"
            | "read_to_string"
            | "read_to_end"
    )
}

/// A field being read or written, named for structs, positional for tuples.
#[derive(Clone)]
pub enum Member {
    Named(Arc<str>),
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
    pub shape: Arc<StructShape>,
    /// Whether a trailing `..rest` value sits in the register after the fields.
    pub has_rest: bool,
}

#[derive(Clone)]
pub struct EnumVariant {
    pub enum_name: Arc<str>,
    pub variant: Arc<str>,
}

/// A method name with its builtin id resolved once at compile time, so hot
/// dispatch matches an enum instead of comparing strings.
#[derive(Clone)]
pub struct MethodName {
    pub text: String,
    pub id: BuiltinId,
    /// The scalar type of an explicit turbofish, `s.parse::<u8>()` for one.
    /// Names are allocated per call site, so this rides along without an
    /// extra operand. Without it `parse` had to guess its target from the
    /// text, which made `"300".parse::<u8>()` an `Ok(300)`.
    pub scalar: Option<ScalarTy>,
}

/// A concrete type a turbofish can name, for the methods whose result type is
/// chosen by the caller rather than by the receiver. The containers nest so a
/// payload stays readable through more than one layer, which is what
/// `Some(None::<f64>).unwrap_or_default().unwrap_or_default()` needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ScalarTy {
    Int(super::numeric::IntWidth),
    F32,
    F64,
    Bool,
    Char,
    Str,
    /// `Option<T>`. Its own `Default` is `None`, and `T` is what one more
    /// unwrap answers with.
    Opt(Box<ScalarTy>),
    /// `Vec<T>`, whose `Default` is the empty vec.
    List(Box<ScalarTy>),
    /// `HashMap<K, V>` or `BTreeMap<K, V>`, whose `Default` is the empty map.
    /// The payload is the value type `V`, which is what `map.get(k)` answers
    /// an `Option` of.
    Map(Box<ScalarTy>),
    /// `HashSet<T>` or `BTreeSet<T>` with its element type, whose `Default`
    /// is the empty set.
    Set(Box<ScalarTy>),
    /// A type the source named but this model does not describe, a user
    /// struct for one. Only its presence matters, never its identity.
    Other,
}

impl ScalarTy {
    /// Lower a turbofish type argument, or `None` for anything that is not one
    /// of these scalars.
    pub fn lower(ty: &syn::Type) -> Option<Self> {
        let syn::Type::Path(path) = ty else {
            return None;
        };
        Self::lower_segment(path.path.segments.last()?)
    }

    /// Lower one path segment, so a `HashMap::<K, V>::new()` call path can be
    /// read without rebuilding a `syn::Type` around its container segment.
    pub fn lower_segment(segment: &syn::PathSegment) -> Option<Self> {
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            // A container knows its own `Default` whatever it holds, and the
            // element type is still carried so one more unwrap can read it.
            let inner = || Box::new(Self::first_arg(args).unwrap_or(Self::Other));
            return match segment.ident.to_string().as_str() {
                "Option" => Some(Self::Opt(inner())),
                "Vec" | "VecDeque" => Some(Self::List(inner())),
                // A map's payload is its value type, the second argument.
                "HashMap" | "BTreeMap" => Some(Self::Map(Box::new(
                    Self::nth_arg(args, 1).unwrap_or(Self::Other),
                ))),
                "HashSet" | "BTreeSet" => Some(Self::Set(inner())),
                _ => None,
            };
        }
        Some(match segment.ident.to_string().as_str() {
            "f32" => Self::F32,
            "f64" => Self::F64,
            "bool" => Self::Bool,
            "char" => Self::Char,
            "String" | "str" => Self::Str,
            name => Self::Int(super::numeric::IntWidth::parse(name)?),
        })
    }

    /// The first type argument of a generic path segment.
    fn first_arg(args: &syn::AngleBracketedGenericArguments) -> Option<Self> {
        Self::nth_arg(args, 0)
    }

    /// The n-th type argument of a generic path segment, lowered.
    fn nth_arg(args: &syn::AngleBracketedGenericArguments, n: usize) -> Option<Self> {
        args.args
            .iter()
            .filter_map(|arg| match arg {
                syn::GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .nth(n)
            .and_then(Self::lower)
    }

    /// What one more unwrap of this type answers with, for a chain like
    /// `Some(None::<f64>).unwrap_or_default().unwrap_or_default()`.
    pub fn payload(&self) -> Option<&ScalarTy> {
        match self {
            Self::Opt(inner) | Self::List(inner) => Some(inner),
            _ => None,
        }
    }
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
    SplitFirst,
    Contains,
    Sort,
    Join,
    Concat,
    Sum,
    Product,
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
    FindMap,
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
        use BuiltinId::{
            All, AndModify, AndThen, Any, Chars, Clone, CloneFrom, Concat, Contains, ContainsKey,
            Copied, Count, EndsWith, Entry, Enumerate, Filter, FilterMap, Find, FindMap, First,
            FlatMap, Fold, ForEach, Get, Insert, IsEmpty, Iter, IterMut, Join, Keys, Last, Len,
            Lines, Map, MapErr, MapOr, MaxByKey, MinByKey, OkOrElse, OrInsertWith, OrInsertWithKey,
            Other, Parse, Partition, Pop, Position, Product, Push, PushStr, Reduce, Remove, Retain,
            Rev, Skip, SkipWhile, Sort, SortBy, SortByCachedKey, SortByKey, Split, SplitFirst,
            SplitWhitespace, StartsWith, Sum, Take, TakeWhile, Then, ToString, Trim, Unwrap,
            UnwrapOr, UnwrapOrElse, Values, WithContext,
        };
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
            "keys" | "into_keys" => Keys,
            "values" | "into_values" => Values,
            "iter" | "into_iter" => Iter,
            "iter_mut" => IterMut,
            "push" => Push,
            "pop" => Pop,
            "first" | "first_mut" => First,
            "last" | "last_mut" => Last,
            "split_first" => SplitFirst,
            "contains" => Contains,
            "sort" | "sort_unstable" => Sort,
            "join" => Join,
            "concat" => Concat,
            "sum" => Sum,
            "product" => Product,
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
            "find_map" => FindMap,
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

    /// The checker-visible receivers this id-dispatched method works on:
    /// the types the coverage walk can infer, `Str`, `Vec`, `Map`, and
    /// `Option`, or `*` for every receiver. Names the interpreter answers
    /// only on iterators, results, entries, or scalars list nothing here,
    /// because the walk never infers those types and checks them by name.
    /// This is what stops a `Vec`-only name from vouching for a `String`.
    pub fn receivers(self) -> &'static [&'static str] {
        use BuiltinId::{
            AndThen, Chars, Clone, CloneFrom, Concat, Contains, ContainsKey, Copied, EndsWith,
            Entry, Filter, First, Get, Insert, IsEmpty, Iter, IterMut, Join, Keys, Last, Len,
            Lines, Map, MapOr, OkOrElse, Parse, Pop, Push, PushStr, Remove, Retain, Sort, SortBy,
            SortByCachedKey, SortByKey, Split, SplitFirst, SplitWhitespace, StartsWith, Take,
            ToString, Trim, Unwrap, UnwrapOr, UnwrapOrElse, Values,
        };
        match self {
            Clone | ToString | CloneFrom => &["*"],
            Len | IsEmpty | Get => &["Str", "Vec", "Map"],
            Insert | Remove => &["Vec", "Map"],
            ContainsKey | Entry | Keys | Values => &["Map"],
            Iter => &["Vec", "Map", "Option"],
            IterMut | Pop | First | Last | SplitFirst | Sort | SortByKey | SortByCachedKey
            | SortBy | Join | Concat | Retain => &["Vec"],
            Push | Contains => &["Str", "Vec"],
            PushStr | SplitWhitespace | Split | Chars | Lines | Trim | StartsWith | EndsWith
            | Parse => &["Str"],
            Take | Unwrap | UnwrapOr | UnwrapOrElse | Copied | Map | Filter | AndThen | MapOr
            | OkOrElse => &["Option"],
            _ => &[],
        }
    }

    /// Whether this method takes a closure and must run through the
    /// interpreter's higher-order dispatch.
    pub fn is_higher_order(self) -> bool {
        use BuiltinId::{
            All, AndModify, AndThen, Any, Filter, FilterMap, Find, FindMap, FlatMap, Fold, ForEach,
            Map, MapErr, MapOr, MaxByKey, MinByKey, OkOrElse, OrInsertWith, OrInsertWithKey, Other,
            Partition, Position, Reduce, Retain, SkipWhile, SortBy, SortByCachedKey, SortByKey,
            TakeWhile, Then, UnwrapOrElse, WithContext,
        };
        matches!(
            self,
            Then | Map
                | Filter
                | FilterMap
                | FlatMap
                | ForEach
                | Find
                | FindMap
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
    /// A width-tagged integer literal: a suffixed one, an annotated one, or a
    /// bare literal past `i64::MAX`, which real Rust can only type as u64 or
    /// usize. `v` is the storage form of `super::numeric::IntWidth`.
    LoadIntW {
        dst: Reg,
        v: i64,
        w: super::numeric::IntWidth,
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
    /// Forget the capture cell of a mutably captured local, so the next use
    /// builds a fresh one from the register. Sits at every binding site of
    /// such a local, which is what makes a `let` inside a loop bind a new
    /// variable each iteration instead of writing on the last one's cell.
    DropCell {
        cell: Reg,
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
    /// Sits right before a `while` or `loop` head, carrying the position of
    /// the loop's own backward jump, so the scalar while plan can take over
    /// at loop entry instead of after the first generic iteration. Falls
    /// through into the head when the loop has no plan.
    LoopHead {
        jump: u32,
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
    /// A compound assignment through a dereference, `*r op= v`. Fused into
    /// one op so a scalar read-modify-write can hold the referent's lock
    /// once, which keeps concurrent compound assignments through a shared
    /// cell from losing updates, see `deref_bin_assign`.
    DerefBinAssign {
        target: Reg,
        val: Reg,
        op: BinKind,
    },
    /// `*param = v` where `param` is a `&mut` function parameter. Sets
    /// through a real reference when the register holds one, otherwise
    /// writes the register itself, which the caller's `&mut` arg writeback
    /// hands back on return. Scalars arrive as plain copies rather than
    /// references, so the strict op has nothing to assign through.
    SetDerefParam {
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

    /// Split the composite in `reg` from any sharing before an in-place
    /// mutation, see `Value::make_unique`. Emitted at every mutable access
    /// so a value clone is a cheap refcount bump until someone writes.
    UniqueReg {
        reg: Reg,
    },
    /// Make the field's current value unique inside `base`'s storage and
    /// load it into `dst` still sharing that storage, so a mutation through
    /// `dst` lands in the field. `base` must already be unique.
    UniqueField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// The indexed-element version of `UniqueField`.
    UniqueIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// Make the value inside a promoted local's cell unique and load it
    /// into `dst` still sharing the cell's storage.
    UniqueCell {
        dst: Reg,
        cell: Reg,
    },
    /// The captured-variable version of `UniqueCell`.
    UniqueUpvalue {
        dst: Reg,
        idx: u16,
    },
    /// Materialize `&mut base[key]` as a real reference value, so writes
    /// through the borrow land in the element.
    RefIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// Materialize `&mut base.field` as a real reference value.
    RefField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// Wrap a place-loaded value as a mutable borrow of its own storage,
    /// for a `&mut place` match scrutinee. Pattern bindings through it
    /// borrow instead of copying, see `test_bind`.
    MakeBorrow {
        dst: Reg,
        src: Reg,
    },
    /// A fresh value of the same shape as `src`, for `mem::take` and
    /// `RefCell::take`, whose `T::default()` has no type to read at runtime.
    DefaultOf {
        dst: Reg,
        src: Reg,
    },
    /// Run user `Drop` impls for a scope's bindings, in reverse declaration
    /// order, at the point the scope ends. `list` indexes the chunk's
    /// `drop_lists`. Emitted only when the program has a `Drop` impl at all.
    DropScope {
        list: u16,
    },
    /// Clear `src` when the value it holds has a user `Drop` impl. Emitted
    /// after a by-value argument copy, so the new holder is the last one
    /// and the guard drops where the move sent it, not at the caller's
    /// scope end. `Drop` types are never `Copy`, so a later use of the
    /// moved name cannot exist in a script that passed the real checker.
    MoveOut {
        src: Reg,
    },

    /// The `?` operator. Unwraps Ok/Some into `dst`, or returns early on Err/None.
    Try {
        dst: Reg,
        src: Reg,
    },
    /// The `?` operator in a program with `Drop` impls. Ok/Some jumps to
    /// `to` with the unwrapped payload in `dst`. Err/None falls through
    /// with the early-return value in `dst`, into the scope drops and the
    /// `Ret` the compiler emits at the site, so `?` cannot skip drops.
    TryJump {
        dst: Reg,
        src: Reg,
        to: u32,
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

    /// Spawn child closure `child` as a tokio task, writing a `JoinHandle` into
    /// `dst`. Emitted only for `#[tokio::main]` scripts.
    Spawn {
        dst: Reg,
        child: u16,
    },
    /// Await the future or `JoinHandle` in `src`, writing its result into `dst`.
    /// Emitted only for `#[tokio::main]` scripts.
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
    /// The written type of each parameter, last path segment only, for example
    /// `Value` for a `&serde_json::Value`. None where the parameter is `self`
    /// or its type is not a plain path. The coverage check reads these, so a
    /// method called on a parameter is checked against the type the author
    /// wrote rather than falling back to a name-only answer.
    pub param_types: Vec<Option<String>>,
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
    pub casts: Vec<CastIr>,
    /// Annotated `let` coercion targets, referenced by `Coerce`.
    pub coerces: Vec<TypeIr>,
    /// Path calls, the segments plus an optional turbofish coercion type.
    pub paths: Vec<(Vec<String>, Option<TypeIr>)>,
    pub names: Vec<MethodName>,
    /// Nested closure bodies, referenced by `MakeClosure`.
    pub children: Vec<Arc<Chunk>>,
    /// For each child, where to copy its upvalues from.
    pub child_caps: Vec<Vec<CapSource>>,
    /// Generic parameter names of this function, in order, e.g. `["T"]`. Used
    /// to bind a caller's turbofish type args when the body resolves them.
    pub generics: Vec<Arc<str>>,
    /// Register lists for `DropScope`, one per scope that has bindings.
    pub drop_lists: Vec<Arc<[Reg]>>,
    /// Turbofish type args recorded at `CallFn` sites, referenced by `targ`.
    pub call_type_args: Vec<Arc<[TypeIr]>>,
    /// True for the synthesized body behind a path used as a function value.
    /// Its arity is a guess, a builtin reference like `u8::saturating_add`
    /// has no known parameter count, so a call with a different count rebuilds
    /// the body for the count actually passed instead of failing.
    pub path_forwarder: bool,
    /// Scalar plans for the `for` loops in this chunk, keyed by the index of
    /// each `ForNext` op and built lazily on the loop's first entry. `None`
    /// records a loop that was analyzed and does not qualify, so the attempt
    /// happens once per loop.
    pub loop_plans: Mutex<HashMap<usize, Option<Arc<super::scalar_loop::LoopPlan>>>>,
    /// Scalar plans for the backward-jump loops in this chunk, `while` and
    /// `loop`, keyed by the index of each closing `Jump` op and built lazily
    /// on the jump's first execution.
    pub while_plans: Mutex<HashMap<usize, Arc<super::scalar_while::WhilePlan>>>,
    /// One flag per op, nonzero when the while plan keyed by that op index
    /// was analyzed and does not qualify. A rejected loop's backward jump
    /// runs per iteration, so the answer has to cost an atomic load, not
    /// the mutex probe of the plan map.
    pub while_rejected: Vec<AtomicU8>,
    /// Scalar plan for this whole function body, built lazily on its first
    /// direct call, see `scalar_fn`.
    pub fn_plan: Mutex<Option<Arc<super::scalar_fn::FnPlan>>>,
    /// Nonzero when the function plan was analyzed and does not qualify, or
    /// failed too often. A rejected function's direct calls pay one atomic
    /// load each, not the mutex probe.
    pub fn_rejected: AtomicU8,
}

impl Chunk {
    pub fn empty(name: impl Into<String>) -> Chunk {
        Chunk {
            code: Vec::new(),
            lines: Vec::new(),
            file: Arc::from(""),
            num_regs: 0,
            num_params: 0,
            param_types: Vec::new(),
            name: name.into(),
            module: 0,
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
            generics: Vec::new(),
            drop_lists: Vec::new(),
            call_type_args: Vec::new(),
            path_forwarder: false,
            loop_plans: Mutex::new(HashMap::new()),
            while_plans: Mutex::new(HashMap::new()),
            while_rejected: Vec::new(),
            fn_plan: Mutex::new(None),
            fn_rejected: AtomicU8::new(0),
        }
    }
}

/// Field layout of a struct, shared by every instance built from the same
/// site. Instances then carry a plain `Vec<Value>` in this order, so a field
/// read is a short name scan plus an index, not a hash probe, and building an
/// instance allocates no map.
pub struct StructShape {
    pub name: Arc<str>,
    pub fields: Vec<Arc<str>>,
    /// One entry per field, its `#[serde(rename = "..")]` name if any. Empty
    /// when the struct has no renamed fields. Read when serializing to json so
    /// the output key matches serde, the same names deserialize already honors.
    pub renames: Vec<Option<Arc<str>>>,
}

impl StructShape {
    pub fn new(name: impl Into<Arc<str>>, fields: Vec<Arc<str>>) -> Arc<StructShape> {
        Arc::new(StructShape {
            name: name.into(),
            fields,
            renames: Vec::new(),
        })
    }

    pub fn with_renames(
        name: impl Into<Arc<str>>,
        fields: Vec<Arc<str>>,
        renames: Vec<Option<Arc<str>>>,
    ) -> Arc<StructShape> {
        Arc::new(StructShape {
            name: name.into(),
            fields,
            renames,
        })
    }

    /// Slot index of a field. Structs have a handful of fields, so a linear
    /// scan beats hashing.
    pub fn slot(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|f| &**f == field)
    }
}

/// The chunk behind a path used as a function value. It forwards its
/// `num_params` arguments to the path call. `num_params` is 0 for a
/// constructor like `Vec::new` and 1 for a method reference or one-arg
/// constructor. A builtin method reference has no known parameter count, so
/// `num_params` is only a default, a call with a different count rebuilds the
/// forwarder for the count actually passed.
pub fn path_call_chunk(segs: Vec<String>, num_params: usize) -> Arc<Chunk> {
    let count = u16::try_from(num_params).expect("parameter count fits u16");
    let dst = count * 2;
    let mut chunk = Chunk::empty("<pathfn>");
    chunk.path_forwarder = true;
    chunk.num_params = num_params;
    chunk.num_regs = num_params * 2 + 1;
    chunk.paths.push((segs, None));
    // Arguments land in registers 0..count. The path call consumes its
    // window, and the frame loop hands the parameter registers back to the
    // caller on return, so the call runs on copies in count..2*count and
    // the result goes just past them.
    for i in 0..count {
        chunk.code.push(Op::Move {
            dst: count + i,
            src: i,
        });
    }
    chunk.code.push(Op::CallPath {
        dst,
        path: 0,
        base: count,
        argc: count,
    });
    chunk.code.push(Op::Ret { src: dst });
    Arc::new(chunk)
}
