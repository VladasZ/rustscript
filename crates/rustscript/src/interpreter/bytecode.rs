//! The compiled form a script runs as. Registers are numbered slots in a flat
//! frame, so variable access is an array read.

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use parking_lot::Mutex;

use super::enum_def::EnumDef;
use super::numeric::IntWidth;
use super::typeir::{CastIr, TypeIr};

pub type Reg = u16;

/// Only literals that need a side table. Integers, booleans and unit have
/// inline load ops.
#[derive(Clone)]
pub enum Const {
    /// i128 exact or u128 as reinterpreted bits.
    Big(i128, IntWidth),
    Float(f64),
    /// Parsed at f32 precision so the value never detours through f64.
    F32(f32),
    Char(char),
    Str(Arc<str>),
    /// `b"..."`, built into a vec of integers.
    Bytes(Arc<[u8]>),
}

/// Sentinel destination for a discarded result, so a map insert skips the
/// `Some(old)` nobody reads.
pub const NO_CONV: u16 = u16::MAX;

pub const DISCARD: Reg = Reg::MAX;

/// Separate from `syn` so the hot loop carries no parse tree types.
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

/// The verb in the real overflow panic.
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

#[derive(Clone)]
pub enum Member {
    Named(FieldName),
    Indexed(usize),
}

/// Caches the slot the last access found. A site that always sees the same
/// type pays one compare per access.
pub struct FieldName {
    pub name: Arc<str>,
    slot: AtomicU16,
}

impl FieldName {
    pub fn new(name: Arc<str>) -> FieldName {
        FieldName {
            name,
            slot: AtomicU16::new(u16::MAX),
        }
    }

    pub fn slot_in(&self, shape: &StructShape) -> Option<usize> {
        let hint = self.slot.load(Ordering::Relaxed);
        if let Some(field) = shape.fields.get(usize::from(hint))
            && (Arc::ptr_eq(field, &self.name) || **field == *self.name)
        {
            return Some(usize::from(hint));
        }
        let found = shape.slot(&self.name)?;
        if let Ok(small) = u16::try_from(found) {
            self.slot.store(small, Ordering::Relaxed);
        }
        Some(found)
    }
}

impl Clone for FieldName {
    fn clone(&self) -> FieldName {
        FieldName {
            name: self.name.clone(),
            slot: AtomicU16::new(self.slot.load(Ordering::Relaxed)),
        }
    }
}

impl Display for FieldName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Clone, Copy)]
pub enum CapSource {
    Local(Reg),
    Upvalue(u16),
    MutableLocal(Reg),
    MutableUpvalue(u16),
}

impl CapSource {
    pub fn is_mutable(self) -> bool {
        matches!(self, Self::MutableLocal(_) | Self::MutableUpvalue(_))
    }
}

/// Fields already in declaration order so serialization matches the
/// compiler. Built once and shared by every instance.
pub struct StructLit {
    /// True when the literal wrote the field, the rest fills the others.
    pub filled: Arc<[bool]>,
    pub shape: Arc<StructShape>,
    /// A `..rest` value sits in the register after the fields.
    pub has_rest: bool,
}

#[derive(Clone)]
pub struct EnumVariant {
    pub def: Arc<EnumDef>,
    pub variant: u16,
}

/// Lowered from the written type at compile time, the runtime has no types
/// to ask.
#[derive(Clone)]
pub enum DefaultIr {
    Int(IntWidth),
    F32,
    F64,
    Bool,
    Char,
    Str,
    Unit,
    Vec,
    Map,
    Set,
    Opt,
    Tuple(Vec<DefaultIr>),
    /// A derived `Default`.
    Struct {
        shape: Arc<StructShape>,
        fields: Vec<DefaultIr>,
    },
    /// The `#[default]` variant.
    Enum(EnumVariant),
}

/// The `type_id` of a value no script type declares.
pub const NO_TYPE: u16 = u16::MAX;
pub const NO_ATOM: u32 = u32::MAX;

#[derive(Clone)]
pub struct MethodName {
    pub text: String,
    pub id: BuiltinId,
    /// See `impls::method_atoms`. `NO_ATOM` otherwise.
    pub atom: u32,
    /// The turbofish scalar, `s.parse::<u8>()`. Without it `parse` guessed
    /// and `"300".parse::<u8>()` was `Ok(300)`.
    pub scalar: Option<ScalarTy>,
    /// The written type behind an `unwrap_or_default`. Covers what
    /// `ScalarTy` cannot, tuples and user types.
    pub default: Option<Arc<DefaultIr>>,
    /// Whether the receiver is a writable place. `String::clear` against the
    /// colored `clear` takes the mutating one only on a place.
    pub place: bool,
}

impl MethodName {
    /// For an internal call that reuses a bridge method.
    pub fn builtin(id: BuiltinId) -> MethodName {
        MethodName {
            text: id.name().to_string(),
            id,
            atom: NO_ATOM,
            scalar: None,
            default: None,
            place: false,
        }
    }
}

impl Display for MethodName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A type a turbofish can name. Containers nest so
/// `Some(None::<f64>).unwrap_or_default().unwrap_or_default()` works.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ScalarTy {
    Int(IntWidth),
    F32,
    F64,
    Bool,
    Char,
    Str,
    /// `Option<T>`.
    Opt(Box<ScalarTy>),
    /// `Vec<T>`.
    List(Box<ScalarTy>),
    /// `HashMap<K, V>` or `BTreeMap<K, V>`. The payload is `V`.
    Map(Box<ScalarTy>),
    /// `HashSet<T>` or `BTreeSet<T>`.
    Set(Box<ScalarTy>),
    /// A type this model does not describe, only its presence matters.
    Other,
}

impl ScalarTy {
    pub fn lower(ty: &syn::Type) -> Option<Self> {
        let syn::Type::Path(path) = ty else {
            return None;
        };
        Self::lower_segment(path.path.segments.last()?)
    }

    /// So `HashMap::<K, V>::new()` reads without rebuilding a `syn::Type`.
    pub fn lower_segment(segment: &syn::PathSegment) -> Option<Self> {
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            // The element type is carried so one more unwrap can read it.
            let inner = || Box::new(Self::first_arg(args).unwrap_or(Self::Other));
            return match segment.ident.to_string().as_str() {
                "Option" => Some(Self::Opt(inner())),
                "Vec" | "VecDeque" => Some(Self::List(inner())),
                // A map's payload is its value type.
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
            name => Self::Int(IntWidth::parse(name)?),
        })
    }

    fn first_arg(args: &syn::AngleBracketedGenericArguments) -> Option<Self> {
        Self::nth_arg(args, 0)
    }

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

    /// What one more unwrap answers with.
    pub fn payload(&self) -> Option<&ScalarTy> {
        match self {
            Self::Opt(inner) | Self::List(inner) => Some(inner),
            _ => None,
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/builtin_id.rs"));
include!(concat!(env!("OUT_DIR"), "/path_id.rs"));

/// `id` is `Other` for a user item the VM looks up by `segs`.
#[derive(Clone)]
pub struct PathRef {
    pub id: PathId,
    pub segs: Vec<String>,
    pub coerce: Option<TypeIr>,
}

impl PathRef {
    pub fn new(segs: Vec<String>, coerce: Option<TypeIr>) -> Self {
        PathRef {
            id: PathId::resolve(&segs),
            segs,
            coerce,
        }
    }

    /// A user item path, never the table.
    pub fn user(segs: Vec<String>, coerce: Option<TypeIr>) -> Self {
        PathRef {
            id: PathId::Other,
            segs,
            coerce,
        }
    }

    pub fn display(&self) -> String {
        self.segs.join("::")
    }
}

impl BuiltinId {
    /// The receivers the coverage walk can infer, `Str`, `Vec`, `Map`,
    /// `Option`, or `*`. This stops a `Vec` only name from vouching for a
    /// `String`.
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

    /// The plain builtins listed here never take a closure, so they skip the
    /// higher order dispatch.
    pub fn is_higher_order(self) -> bool {
        use BuiltinId::{
            Chars, Clone, CloneFrom, Cloned, Concat, Contains, ContainsKey, Copied, Count,
            EndsWith, Entry, Enumerate, First, FirstMut, Get, GetMut, Insert, IntoIter, IntoKeys,
            IntoValues, IsEmpty, Iter, IterMut, Join, Keys, Last, LastMut, Len, Lines, Parse, Pop,
            Product, Push, PushStr, Remove, Rev, Skip, Sort, SortUnstable, Split, SplitFirst,
            SplitWhitespace, StartsWith, Sum, Take, ToString, Trim, Unwrap, UnwrapOr, Values,
        };
        !matches!(
            self,
            Len | IsEmpty
                | Clone
                | ToString
                | Get
                | GetMut
                | Insert
                | ContainsKey
                | Remove
                | Entry
                | Keys
                | IntoKeys
                | Values
                | IntoValues
                | Iter
                | IntoIter
                | IterMut
                | Push
                | Pop
                | First
                | FirstMut
                | Last
                | LastMut
                | SplitFirst
                | Contains
                | Sort
                | SortUnstable
                | Join
                | Concat
                | Sum
                | Product
                | Enumerate
                | Rev
                | Count
                | Take
                | Skip
                | PushStr
                | CloneFrom
                | SplitWhitespace
                | Split
                | Chars
                | Lines
                | Trim
                | StartsWith
                | EndsWith
                | Parse
                | Unwrap
                | UnwrapOr
                | Copied
                | Cloned
        )
    }
}

#[derive(Clone)]
pub struct FmtSpec {
    pub template: String,
    pub positional: Vec<Reg>,
    pub named: Vec<(String, Reg)>,
}

pub struct PatInfo {
    pub pat: PPat,
    pub binds: Vec<(String, Reg)>,
}

/// `variant` is set when the path resolved to a definition. `name` is kept
/// for the shapes that match by name alone, a tuple struct, the
/// `serde_json::Value` variants held as plain values, an unresolved path.
#[derive(Clone)]
pub struct PTag {
    pub name: Option<Arc<str>>,
    pub variant: Option<(Arc<EnumDef>, u16)>,
}

impl PTag {
    pub fn matches(&self, def: &Arc<EnumDef>, variant: u16) -> bool {
        match &self.variant {
            Some((want, index)) => EnumDef::same(want, def) && *index == variant,
            None => self.name.as_deref() == Some(&**def.variant_name(variant)),
        }
    }

    pub fn is_named(&self, name: &str) -> bool {
        self.name.as_deref() == Some(name)
    }
}

#[derive(Clone)]
pub enum PLit {
    /// Wide enough for `u64::MAX` and the 128 bit widths.
    Int(i128),
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
        tag: PTag,
        elems: Vec<PPat>,
    },
    Path {
        tag: PTag,
    },
    Struct {
        name: Option<String>,
        fields: Vec<(String, PPat)>,
    },
    Or(Vec<PPat>),
    Slice(Vec<PPat>),
    /// A missing endpoint leaves that side unbounded.
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
    /// A width tagged integer literal. `v` is the storage form of
    /// `super::numeric::IntWidth`.
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
    /// Forget the capture cell at a binding site, so a `let` inside a loop
    /// binds a new variable each iteration.
    DropCell {
        cell: Reg,
    },
    StoreUpvalue {
        idx: u16,
        src: Reg,
    },
    /// Evaluated lazily on first use.
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
    /// `n - 1`, `i < len`.
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
    /// Carries the loop's backward jump, so the while plan can take over at
    /// loop entry instead of after the first generic iteration.
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
    /// Jump to `to` when `a op b` is false.
    CmpJump {
        a: Reg,
        b: Reg,
        op: BinKind,
        to: u32,
    },
    /// Against an integer literal.
    CmpJumpImm {
        a: Reg,
        imm: i64,
        op: BinKind,
        to: u32,
    },

    /// `targ` indexes `call_type_args`, or `u32::MAX` without a turbofish.
    CallFn {
        dst: Reg,
        func: u32,
        base: Reg,
        argc: u16,
        targ: u32,
    },
    CallValue {
        dst: Reg,
        callee: Reg,
        base: Reg,
        argc: u16,
    },
    /// Any other call, resolved by path.
    CallPath {
        dst: Reg,
        path: u16,
        base: Reg,
        argc: u16,
    },
    /// `None`, a unit variant, `consts::OS`.
    PathValue {
        dst: Reg,
        path: u16,
    },
    Method {
        dst: Reg,
        recv: Reg,
        name: u16,
        base: Reg,
        argc: u16,
    },
    /// Fused `recv.get(key).copied().unwrap_or(default)`, one probe. Falls
    /// back to the 3 real methods off a map or vec.
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
    /// `new` and `with_capacity` lowered in place.
    MakeMap {
        dst: Reg,
        set: bool,
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
    IterInit {
        dst: Reg,
        src: Reg,
    },
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
    /// `*r op= v`, fused so the read-modify-write holds the lock once and
    /// concurrent tasks cannot lose updates.
    DerefBinAssign {
        target: Reg,
        val: Reg,
        op: BinKind,
    },
    /// `*param = v` on a `&mut` parameter. Scalars arrive as copies, so this
    /// writes the register and the `&mut` writeback hands it back.
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

    /// Split from sharing before a mutation, see `Value::make_unique`.
    UniqueReg {
        reg: Reg,
    },
    /// Load the field into `dst` still sharing `base`'s storage, so a
    /// mutation lands in the field. `base` must already be unique.
    UniqueField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// `UniqueField` for an element.
    UniqueIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `UniqueField` for a promoted local's cell.
    UniqueCell {
        dst: Reg,
        cell: Reg,
    },
    /// `UniqueCell` for a captured variable.
    UniqueUpvalue {
        dst: Reg,
        idx: u16,
    },
    /// `&mut base[key]` as a real reference value.
    RefIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `&mut base.field` as a real reference value.
    RefField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// For a `&mut place` match scrutinee, so bindings borrow instead of
    /// copying, see `test_bind`.
    MakeBorrow {
        dst: Reg,
        src: Reg,
    },
    /// For `mem::take` and `RefCell::take`, whose `T::default()` has no type
    /// at runtime.
    DefaultOf {
        dst: Reg,
        src: Reg,
    },
    /// `ir` indexes `defaults`.
    BuildDefault {
        dst: Reg,
        ir: u16,
    },
    /// `list` indexes `drop_lists`. Emitted only when the program has a
    /// `Drop` impl.
    DropScope {
        list: u16,
    },
    /// After a by value argument copy, so the guard drops where the move
    /// sent it. `Drop` types are never `Copy`, so the checker rules out a
    /// later use.
    MoveOut {
        src: Reg,
    },

    /// `conv` indexes `try_targets` with the function's error type, or
    /// `NO_CONV` when the error leaves as it is.
    Try {
        dst: Reg,
        src: Reg,
        conv: u16,
    },
    /// `?` with `Drop` impls. Err falls through into the scope drops and the
    /// `Ret` at the site, so `?` cannot skip drops.
    TryJump {
        dst: Reg,
        src: Reg,
        to: u32,
        conv: u16,
    },
    Cast {
        dst: Reg,
        src: Reg,
        ty: u16,
    },
    /// `let c: Config = ..`.
    Coerce {
        dst: Reg,
        src: Reg,
        ty: u16,
    },

    /// `dst` receives a bool.
    TestBind {
        val: Reg,
        pat: u16,
        dst: Reg,
    },

    Fmt {
        dst: Reg,
        spec: u16,
    },
    MacroCall {
        kind: MacroKind,
        dst: Reg,
        spec: u16,
    },
    /// `dbg!` takes plain registers.
    Dbg {
        dst: Reg,
        base: Reg,
        argc: u16,
    },

    /// `#[tokio::main]` only.
    Spawn {
        dst: Reg,
        child: u16,
    },
    /// `#[tokio::main]` only.
    Await {
        dst: Reg,
        src: Reg,
    },
}

pub struct Chunk {
    pub code: Vec<Op>,
    /// Parallel to `code`. Zero means unknown.
    pub lines: Vec<u32>,
    /// Shown in error traces.
    pub file: Arc<str>,
    pub num_regs: usize,
    pub num_params: usize,
    /// Last path segment only, `Value` for a `&serde_json::Value`. The
    /// coverage check reads these.
    pub param_types: Vec<Option<String>>,
    pub name: String,
    /// For runtime type resolution.
    pub module: u16,
    /// A `move` closure gives a mutable capture its own cell.
    pub moves: bool,

    // Side tables referenced by instruction operands.
    pub consts: Vec<Const>,
    pub members: Vec<Member>,
    pub pats: Vec<PatInfo>,
    pub fmts: Vec<FmtSpec>,
    pub struct_lits: Vec<StructLit>,
    pub enum_variants: Vec<EnumVariant>,
    pub casts: Vec<CastIr>,
    pub defaults: Vec<DefaultIr>,
    pub try_targets: Vec<Arc<str>>,
    pub coerces: Vec<TypeIr>,
    pub paths: Vec<PathRef>,
    pub names: Vec<MethodName>,
    pub children: Vec<Arc<Chunk>>,
    pub child_caps: Vec<Vec<CapSource>>,
    /// To bind a caller's turbofish type args.
    pub generics: Vec<Arc<str>>,
    pub drop_lists: Vec<Arc<[Reg]>>,
    pub call_type_args: Vec<Arc<[TypeIr]>>,
    /// A forwarder's arity is a guess, a call with a different count rebuilds
    /// it.
    pub path_forwarder: bool,
    /// Keyed by `ForNext` op index, built on first entry. `None` records a
    /// loop that does not qualify.
    pub loop_plans: Mutex<HashMap<usize, Option<Arc<super::scalar_loop::LoopPlan>>>>,
    /// Keyed by closing `Jump` op index.
    pub while_plans: Mutex<HashMap<usize, Arc<super::scalar_while::WhilePlan>>>,
    /// A rejected loop's backward jump runs per iteration, so the answer must
    /// cost an atomic load and not the mutex probe.
    pub while_rejected: Vec<AtomicU8>,
    /// See `scalar_fn`.
    pub fn_plan: Mutex<Option<Arc<super::scalar_fn::FnPlan>>>,
    /// Same reason as `while_rejected`.
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
            moves: false,
            consts: Vec::new(),
            members: Vec::new(),
            pats: Vec::new(),
            fmts: Vec::new(),
            struct_lits: Vec::new(),
            enum_variants: Vec::new(),
            casts: Vec::new(),
            defaults: Vec::new(),
            try_targets: Vec::new(),
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

/// Shared by every instance, so a field read is a short scan plus an index
/// and building an instance allocates no map.
pub struct StructShape {
    pub name: Arc<str>,
    /// Index into the impl table. `NO_TYPE` for a bridge struct.
    pub type_id: u16,
    pub fields: Vec<Arc<str>>,
    /// `#[serde(rename = "..")]` per field, empty when none. Read when
    /// serializing.
    pub renames: Vec<Option<Arc<str>>>,
}

impl StructShape {
    pub fn new(name: impl Into<Arc<str>>, fields: Vec<Arc<str>>) -> Arc<StructShape> {
        Arc::new(StructShape {
            name: name.into(),
            type_id: NO_TYPE,
            fields,
            renames: Vec::new(),
        })
    }

    pub fn typed(
        name: impl Into<Arc<str>>,
        type_id: u16,
        fields: Vec<Arc<str>>,
        renames: Vec<Option<Arc<str>>>,
    ) -> Arc<StructShape> {
        Arc::new(StructShape {
            name: name.into(),
            type_id,
            fields,
            renames,
        })
    }

    /// A linear scan beats hashing on a handful of fields.
    pub fn slot(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|f| &**f == field)
    }
}

/// The chunk behind a path used as a function value. A builtin reference
/// has no known parameter count, so `num_params` is only a default.
pub fn path_call_chunk(path: PathRef, num_params: usize) -> Arc<Chunk> {
    let count = u16::try_from(num_params).expect("parameter count fits u16");
    let dst = count * 2;
    let mut chunk = Chunk::empty("<pathfn>");
    chunk.path_forwarder = true;
    chunk.num_params = num_params;
    chunk.num_regs = num_params * 2 + 1;
    chunk.paths.push(path);
    // The frame loop hands the parameter registers back on return, so the
    // call runs on copies in count..2*count.
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
