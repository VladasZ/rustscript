//! `Send + Sync` bytecode for the parallel engine. The compiler emits the fast
//! `Rc` based `Chunk`; for a `#[tokio::main]` script we convert that tree once
//! at load into a `PChunk`. The fast `Chunk` is left exactly as it is, so the
//! single threaded path pays nothing for this.
//!
//! syn AST nodes are not `Send` (proc-macro2 spans carry a compiler handle), so
//! nothing here may hold a `syn::Type` or `syn::Pat`. Patterns arrive as the
//! plain `PPat` IR, and cast, coercion, and turbofish tables as the shared
//! `CastIr` and `TypeIr`, all already free of syn.

use std::sync::Arc;

use super::bytecode::{
    CapSource, Chunk, Const, EnumVariant, FmtSpec, Member, MethodName, Op, PPat, PatInfo, StructLit,
};
use super::pvalue::PStructShape;
use super::typeir::{CastIr, TypeIr};

/// A field access, the `Arc` twin of `bytecode::Member`.
#[derive(Clone)]
pub enum PMember {
    Named(Arc<str>),
    Indexed(usize),
}

/// A struct literal shape plus whether a trailing `..rest` follows.
pub struct PStructLit {
    pub shape: Arc<PStructShape>,
    pub has_rest: bool,
}

pub struct PEnumVariant {
    pub enum_name: Arc<str>,
    pub variant: Arc<str>,
}

/// A pattern plus the register each bound name writes into.
pub struct PPatInfo {
    pub pat: PPat,
    pub binds: Vec<(String, u16)>,
}

/// One compiled function, method, or closure body, `Send + Sync`.
pub struct PChunk {
    pub code: Vec<Op>,
    /// Source line of each op, parallel to `code`. Zero means unknown.
    pub lines: Vec<u32>,
    /// Source file this body was written in, shown in runtime error traces.
    pub file: Arc<str>,
    pub num_regs: usize,
    pub num_params: usize,
    pub name: String,

    pub consts: Vec<Const>,
    pub members: Vec<PMember>,
    pub pats: Vec<PPatInfo>,
    pub fmts: Vec<FmtSpec>,
    pub struct_lits: Vec<PStructLit>,
    pub enum_variants: Vec<PEnumVariant>,
    /// `as` cast targets, shared with the fast engine.
    pub casts: Vec<CastIr>,
    /// Annotated `let` coercion targets, referenced by `Coerce`.
    pub coerces: Vec<TypeIr>,
    /// Path calls, the segments plus an optional turbofish coercion type.
    pub paths: Vec<(Vec<String>, Option<TypeIr>)>,
    pub names: Vec<MethodName>,
    pub children: Vec<Arc<PChunk>>,
    pub child_caps: Vec<Vec<CapSource>>,
    /// Generic parameter names, bound to a caller's turbofish types at calls.
    pub generics: Vec<Arc<str>>,
    /// Turbofish type args recorded at `CallFn` sites, referenced by `targ`.
    pub call_type_args: Vec<Arc<[TypeIr]>>,
}

/// Convert a fast `Chunk` tree into an `Arc` based `PChunk` tree. Runs once per
/// script at load.
pub fn convert(chunk: &Chunk) -> Arc<PChunk> {
    Arc::new(PChunk {
        code: chunk.code.clone(),
        lines: chunk.lines.clone(),
        file: chunk.file.clone(),
        num_regs: chunk.num_regs,
        num_params: chunk.num_params,
        name: chunk.name.clone(),
        consts: chunk.consts.clone(),
        members: chunk.members.iter().map(convert_member).collect(),
        pats: chunk.pats.iter().map(convert_pat_info).collect(),
        fmts: chunk.fmts.clone(),
        struct_lits: chunk.struct_lits.iter().map(convert_lit).collect(),
        enum_variants: chunk
            .enum_variants
            .iter()
            .map(convert_enum_variant)
            .collect(),
        casts: chunk.casts.clone(),
        coerces: chunk.coerces.clone(),
        paths: chunk.paths.clone(),
        names: chunk.names.clone(),
        children: chunk.children.iter().map(|c| convert(c)).collect(),
        child_caps: chunk.child_caps.clone(),
        generics: chunk.generics.iter().map(|g| Arc::from(&**g)).collect(),
        call_type_args: chunk.call_type_args.clone(),
    })
}

fn convert_enum_variant(variant: &EnumVariant) -> PEnumVariant {
    PEnumVariant {
        enum_name: Arc::from(&*variant.enum_name),
        variant: Arc::from(&*variant.variant),
    }
}

fn convert_member(m: &Member) -> PMember {
    match m {
        Member::Named(n) => PMember::Named(Arc::from(&**n)),
        Member::Indexed(i) => PMember::Indexed(*i),
    }
}

fn convert_lit(lit: &StructLit) -> PStructLit {
    let shape = &lit.shape;
    let fields: Vec<Arc<str>> = shape.fields.iter().map(|f| Arc::from(&**f)).collect();
    let renames: Vec<Option<Arc<str>>> = shape
        .renames
        .iter()
        .map(|r| r.as_ref().map(|s| Arc::from(&**s)))
        .collect();
    let pshape = Arc::new(PStructShape {
        name: Arc::from(&*shape.name),
        fields,
        renames,
    });
    PStructLit {
        shape: pshape,
        has_rest: lit.has_rest,
    }
}

fn convert_pat_info(info: &PatInfo) -> PPatInfo {
    PPatInfo {
        pat: info.pat.clone(),
        binds: info.binds.clone(),
    }
}
