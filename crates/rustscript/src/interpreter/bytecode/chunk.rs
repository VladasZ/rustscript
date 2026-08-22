//! The `Op` set, the `Chunk` a function compiles to and the struct shapes it builds.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use parking_lot::Mutex;

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

    Jump {
        to: u32,
    },
    /// Carries the backward jump of the loop, so the while plan can take over at loop entry
    /// instead of after the first generic iteration.
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
    IterInit {
        dst: Reg,
        src: Reg,
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

    /// split from sharing before a mutation, see `Value::make_unique`
    UniqueReg {
        reg: Reg,
    },
    /// Load the field into `dst` still sharing the storage of `base`, so a mutation lands in the
    /// field. `base` must already be unique.
    UniqueField {
        dst: Reg,
        base: Reg,
        member: u16,
    },
    /// `UniqueField` for an element
    UniqueIndex {
        dst: Reg,
        base: Reg,
        key: Reg,
    },
    /// `UniqueField` for the cell of a promoted local
    UniqueCell {
        dst: Reg,
        cell: Reg,
    },
    /// `UniqueCell` for a captured variable
    UniqueUpvalue {
        dst: Reg,
        idx: u16,
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
    /// After a by value argument copy, so the guard drops where the move sent it. `Drop` types are
    /// never `Copy`, so the checker rules out a later use.
    MoveOut {
        src: Reg,
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
    /// to bind the turbofish type args of a caller
    pub generics: Vec<Arc<str>>,
    pub drop_lists: Vec<Arc<[Reg]>>,
    pub call_type_args: Vec<Arc<[TypeIr]>>,
    /// A forwarder's arity is a guess, a call with a different count rebuilds it.
    pub path_forwarder: bool,
    /// Keyed by `ForNext` op index, built on first entry. `None` means the loop doesn't qualify.
    pub loop_plans: Mutex<HashMap<usize, Option<Arc<crate::interpreter::scalar_loop::LoopPlan>>>>,
    /// keyed by the closing `Jump` op index
    pub while_plans: Mutex<HashMap<usize, Arc<crate::interpreter::scalar_while::WhilePlan>>>,
    /// The backward jump of a rejected loop runs every iteration, so this must be an atomic load
    /// and not a mutex probe.
    pub while_rejected: Vec<AtomicU8>,
    pub fn_plan: Mutex<Option<Arc<crate::interpreter::scalar_fn::FnPlan>>>,
    /// same reason as `while_rejected`
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
