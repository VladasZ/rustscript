//! The compiled form a script runs as. Registers are numbered slots in a flat frame, so a
//! variable access is an array read.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use super::enum_def::EnumDef;
use super::numeric::IntWidth;
use super::typeir::TypeIr;

pub type Reg = u16;

/// Only literals that need a side table. Integers, bools and unit have inline load ops.
#[derive(Clone)]
pub enum Const {
    /// i128 as is, u128 as reinterpreted bits
    Big(i128, IntWidth),
    Float(f64),
    /// parsed at f32 precision, never goes through f64
    F32(f32),
    Char(char),
    Str(Arc<str>),
    /// `b"..."`, built into a vec of integers
    Bytes(Arc<[u8]>),
}

/// Sentinel destination for a discarded result, so a map insert skips the `Some(old)` nobody reads.
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

/// Caches the slot the last access found. A site that always sees the same type pays 1 compare
/// per access.
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

/// Fields already in declaration order so serialization matches the compiler. Built once, shared
/// by every instance.
pub struct StructLit {
    /// true when the literal wrote the field, the rest fills the others
    pub filled: Arc<[bool]>,
    pub shape: Arc<StructShape>,
    /// a `..rest` value sits in the register after the fields
    pub has_rest: bool,
}

#[derive(Clone)]
pub struct EnumVariant {
    pub def: Arc<EnumDef>,
    pub variant: u16,
}

/// Lowered from the written type at compile time, the runtime has no types to ask.
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
    /// a derived `Default`
    Struct {
        shape: Arc<StructShape>,
        fields: Vec<DefaultIr>,
    },
    /// the `#[default]` variant
    Enum(EnumVariant),
}

/// The `type_id` of a value no script type declares.
pub const NO_TYPE: u16 = u16::MAX;
pub const NO_ATOM: u32 = u32::MAX;

#[derive(Clone)]
pub struct MethodName {
    pub text: String,
    pub id: BuiltinId,
    /// see `impls::method_atoms`, `NO_ATOM` otherwise
    pub atom: u32,
    /// The turbofish scalar, `s.parse::<u8>()`. Without it `parse` guesses and
    /// `"300".parse::<u8>()` is `Ok(300)`.
    pub scalar: Option<ScalarTy>,
    /// The written type behind an `unwrap_or_default`. Covers what `ScalarTy` can't, tuples and
    /// user types.
    pub default: Option<Arc<DefaultIr>>,
    /// Whether the receiver is a writable place. `String::clear` vs the colored `clear`, the
    /// mutating one only on a place.
    pub place: bool,
}

impl MethodName {
    /// for an internal call that reuses a bridge method
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
    /// `Option<T>`
    Opt(Box<ScalarTy>),
    /// `Vec<T>`
    List(Box<ScalarTy>),
    /// `HashMap<K, V>` or `BTreeMap<K, V>`, the payload is `V`
    Map(Box<ScalarTy>),
    /// `HashSet<T>` or `BTreeSet<T>`
    Set(Box<ScalarTy>),
    /// a type this model doesn't describe, only its presence matters
    Other,
}

impl ScalarTy {
    pub fn lower(ty: &syn::Type) -> Option<Self> {
        let syn::Type::Path(path) = ty else {
            return None;
        };
        Self::lower_segment(path.path.segments.last()?)
    }

    /// So `HashMap::<K, V>::new()` can be read without rebuilding a `syn::Type`.
    pub fn lower_segment(segment: &syn::PathSegment) -> Option<Self> {
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            // the element type is kept so one more unwrap can read it
            let inner = || Box::new(Self::first_arg(args).unwrap_or(Self::Other));
            return match segment.ident.to_string().as_str() {
                "Option" => Some(Self::Opt(inner())),
                "Vec" | "VecDeque" => Some(Self::List(inner())),
                // a map's payload is its value type
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

    /// What one more unwrap gives.
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

    /// a user item path, never the table
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
    /// The receivers the coverage walk can infer, `Str`, `Vec`, `Map`, `Option`, or `*`.
    /// So a `Vec` only name doesn't vouch for a `String`.
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

    /// The plain builtins here never take a closure, so they skip the higher order dispatch.
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

/// `variant` is set when the path resolved to a definition. `name` is kept for the shapes that match
/// by name alone, a tuple struct, the `serde_json::Value` variants held as plain values, an
/// unresolved path.
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
    /// wide enough for `u64::MAX` and the 128 bit widths
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
    /// a missing endpoint leaves that side unbounded
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

mod chunk;

pub use chunk::{Chunk, Op, StructShape, path_call_chunk};
