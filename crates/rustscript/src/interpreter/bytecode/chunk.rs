//! The `Op` set, the `Chunk` a function compiles to and the struct shapes it builds.

use std::sync::Arc;

use super::{
    BinKind, CapSource, Const, DefaultIr, EnumVariant, FmtSpec, MacroKind, Member, MethodName,
    NO_TYPE, PatInfo, PathRef, Reg, StructLit, UnKind,
};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::typeir::{CastIr, TypeIr};

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
    /// A width tagged integer literal. `v` is the storage form of `IntWidth`.
    LoadIntW {
        dst: Reg,
        v: i64,
        w: IntWidth,
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
    /// Forget the capture cell at a binding site, so a `let` inside a loop binds a new variable
    /// each iteration.
    DropCell {
        cell: Reg,
    },
    StoreUpvalue {
        idx: u16,
        src: Reg,
    },
    /// lazy, evaluated on first use
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
    /// `n - 1`, `i < len`
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
    /// `Bin` where the inference pass typed both sides as the same integer width. Runs the
    /// width natively, falls back to `Bin` when a value is not what the pass said.
    BinInt {
        dst: Reg,
        a: Reg,
        b: Reg,
        op: BinKind,
        w: IntWidth,
    },
    BinIntImm {
        dst: Reg,
        a: Reg,
        imm: i64,
        op: BinKind,
        w: IntWidth,
    },
    /// `Bin` on 2 floats of the same precision
    BinFloat {
        dst: Reg,
        a: Reg,
        b: Reg,
        op: BinKind,
        f32: bool,
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
    /// jump to `to` when `a op b` is false
    CmpJump {
        a: Reg,
        b: Reg,
        op: BinKind,
        to: u32,
    },
    /// against an integer literal
    CmpJumpImm {
        a: Reg,
        imm: i64,
        op: BinKind,
        to: u32,
    },
    /// `CmpJump` on 2 integers of the same width
    CmpJumpInt {
        a: Reg,
        b: Reg,
        op: BinKind,
        w: IntWidth,
        to: u32,
    },
    CmpJumpIntImm {
        a: Reg,
        imm: i64,
        op: BinKind,
        w: IntWidth,
        to: u32,
    },

    /// `targ` indexes `call_type_args`, `u32::MAX` without a turbofish
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
    /// any other call, resolved by path
    CallPath {
        dst: Reg,
        path: u16,
        base: Reg,
        argc: u16,
    },
    /// `None`, a unit variant, `consts::OS`
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
    /// Fused `recv.get(key).copied().unwrap_or(default)`, 1 probe. Falls back to the 3 real
    /// methods when the receiver is not a map or vec.
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
    /// `new` and `with_capacity` lowered in place
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
    /// `owned` means the loop consumed its source, so the iterator takes the items and drops
    /// what a `break` leaves behind.
    IterInit {
        dst: Reg,
        src: Reg,
        owned: bool,
    },
    /// jumps to `to` when exhausted
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
    /// `*r op= v`, fused so the read-modify-write holds the lock once and concurrent tasks can't
    /// lose updates.
    DerefBinAssign {
        target: Reg,
        val: Reg,
        op: BinKind,
    },
    /// `*param = v` on a `&mut` parameter. Scalars arrive as copies, so this writes the register
    /// and the `&mut` writeback hands it back.
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

    /// A value flowing into an owned position, a `let`, a by value argument, a constructor field,
    /// a return. Compile time only. The liveness pass turns it into `Take` when `root` is dead
    /// after this op, which is a move, and into `Copy` when `root` is still read, which `rustc`
    /// only accepts for a `Copy` type. `root` is `NO_ROOT` for a read with no local behind it.
    Own {
        dst: Reg,
        src: Reg,
        root: Reg,
    },
    /// a move, `src` is cleared
    Take {
        dst: Reg,
        src: Reg,
    },
    /// a deep copy of a composite, see `Value::deep_clone`
    Copy {
        dst: Reg,
        src: Reg,
    },
    /// `&mut base[key]` as a real reference value
    RefIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `&mut base.field` as a real reference value
    RefField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// For a `&mut place` match scrutinee, so bindings borrow instead of copying. See `test_bind`.
    MakeBorrow {
        dst: Reg,
        src: Reg,
    },
    /// For `mem::take` and `RefCell::take`, their `T::default()` has no type at runtime.
    DefaultOf {
        dst: Reg,
        src: Reg,
    },
    /// `ir` indexes `defaults`
    BuildDefault {
        dst: Reg,
        ir: u16,
    },
    /// `list` indexes `drop_lists`. Only emitted when the program has a `Drop` impl.
    DropScope {
        list: u16,
    },

    /// `conv` indexes `try_targets` with the error type of the function, `NO_CONV` when the error
    /// leaves as is.
    Try {
        dst: Reg,
        src: Reg,
        conv: u16,
    },
    /// `?` when there are `Drop` impls. Err falls through into the scope drops and the `Ret` at
    /// the site, so `?` can't skip drops.
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
    /// `let c: Config = ..`
    Coerce {
        dst: Reg,
        src: Reg,
        ty: u16,
    },

    /// `dst` receives a bool
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
    /// `dbg!` takes plain registers
    Dbg {
        dst: Reg,
        base: Reg,
        argc: u16,
    },

    /// `#[tokio::main]` only
    Spawn {
        dst: Reg,
        child: u16,
    },
    /// `#[tokio::main]` only
    Await {
        dst: Reg,
        src: Reg,
    },
}

pub struct Chunk {
    pub code: Vec<Op>,
    /// parallel to `code`, zero means unknown
    pub lines: Vec<u32>,
    /// parallel to `code`, 1 based like `rustc`, zero means unknown
    pub cols: Vec<u32>,
    /// shown in error traces
    pub file: Arc<str>,
    pub num_regs: usize,
    pub num_params: usize,
    /// Last path segment only, `Value` for a `&serde_json::Value`. The coverage check reads these.
    pub param_types: Vec<Option<String>>,
    pub name: String,
    /// for runtime type resolution
    pub module: u16,
    /// a `move` closure gives a mutable capture its own cell
    pub moves: bool,

    // side tables referenced by instruction operands
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
    /// Parallel to `child_caps`. True where a `move` closure takes the local, because the local
    /// is dead after the closure is built. False shares the handle, or copies for a `move`.
    pub child_moves: Vec<Arc<[bool]>>,
    /// to bind the turbofish type args of a caller
    pub generics: Vec<Arc<str>>,
    pub drop_lists: Vec<Arc<[Reg]>>,
    /// Every register a `DropScope` can drop, highest first. Unwinding drops these and nothing
    /// else, a temporary may hold a borrowed handle.
    pub droppable: Arc<[Reg]>,
    pub call_type_args: Vec<Arc<[TypeIr]>>,
    /// A forwarder's arity is a guess, a call with a different count rebuilds it.
    pub path_forwarder: bool,
}

impl Chunk {
    pub fn empty(name: impl Into<String>) -> Chunk {
        Chunk {
            code: Vec::new(),
            lines: Vec::new(),
            cols: Vec::new(),
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
            child_moves: Vec::new(),
            generics: Vec::new(),
            drop_lists: Vec::new(),
            droppable: Arc::from(Vec::new()),
            call_type_args: Vec::new(),
            path_forwarder: false,
        }
    }
}

/// Shared by every instance, so a field read is a short scan plus an index and building an
/// instance allocates no map.
pub struct StructShape {
    pub name: Arc<str>,
    /// index into the impl table, `NO_TYPE` for a bridge struct
    pub type_id: u16,
    pub fields: Vec<Arc<str>>,
    /// `#[serde(rename = "..")]` per field, empty when none. Read when serializing.
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

    /// a linear scan beats hashing on a handful of fields
    pub fn slot(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|f| &**f == field)
    }
}

/// The chunk behind a path used as a function value. A builtin reference has no known parameter
/// count, so `num_params` is only a default.
pub fn path_call_chunk(path: PathRef, num_params: usize) -> Arc<Chunk> {
    let count = u16::try_from(num_params).expect("parameter count fits u16");
    let dst = count * 2;
    let mut chunk = Chunk::empty("<pathfn>");
    chunk.path_forwarder = true;
    chunk.num_params = num_params;
    chunk.num_regs = num_params * 2 + 1;
    chunk.paths.push(path);
    // the frame loop hands the parameter registers back on return, so the call runs on copies in
    // count..2*count
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
