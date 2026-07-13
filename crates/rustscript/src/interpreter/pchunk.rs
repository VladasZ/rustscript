//! `Send + Sync` bytecode for the parallel engine. The compiler emits the fast
//! `Rc` based `Chunk`; for a `#[tokio::main]` script we convert that tree once
//! at load into a `PChunk`. The fast `Chunk` is left exactly as it is, so the
//! single threaded path pays nothing for this.
//!
//! syn AST nodes are not `Send` (proc-macro2 spans carry a compiler handle), so
//! nothing here may hold a `syn::Type` or `syn::Pat`. Patterns are lowered to a
//! plain `PPat` IR, and the type-only side tables the parallel VM does not use
//! (casts, coercions, turbofish args) are dropped in the conversion.

use std::sync::Arc;

use syn::{Lit, Pat, Type};

use super::bytecode::{
    CapSource, Chunk, Const, FmtSpec, Member, MethodName, Op, PatInfo, StructLit,
};
use super::pvalue::PStructShape;

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

/// A pattern plus the register each bound name writes into.
pub struct PPatInfo {
    pub pat: PPat,
    pub binds: Vec<(String, u16)>,
}

/// A `Send` literal used inside patterns.
pub enum PLit {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char),
}

/// A `Send` pattern IR, lowered from `syn::Pat`. Covers the forms the VM binds.
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
    /// A pattern the parallel engine does not lower yet; never matches.
    Unsupported,
}

/// One compiled function, method, or closure body, `Send + Sync`.
pub struct PChunk {
    pub code: Vec<Op>,
    pub num_regs: usize,
    pub num_params: usize,
    pub name: String,

    pub consts: Vec<Const>,
    pub members: Vec<PMember>,
    pub pats: Vec<PPatInfo>,
    pub fmts: Vec<FmtSpec>,
    pub struct_lits: Vec<PStructLit>,
    /// `as` cast targets, reduced to the target type name, e.g. `f64`, `usize`.
    pub casts: Vec<String>,
    /// Path calls, just the segments; the parallel VM resolves types at runtime.
    pub paths: Vec<Vec<String>>,
    pub names: Vec<MethodName>,
    pub children: Vec<Arc<PChunk>>,
    pub child_caps: Vec<Vec<CapSource>>,
}

/// Convert a fast `Chunk` tree into an `Arc` based `PChunk` tree. Runs once per
/// script at load.
pub fn convert(chunk: &Chunk) -> Arc<PChunk> {
    Arc::new(PChunk {
        code: chunk.code.clone(),
        num_regs: chunk.num_regs,
        num_params: chunk.num_params,
        name: chunk.name.clone(),
        consts: chunk.consts.clone(),
        members: chunk.members.iter().map(convert_member).collect(),
        pats: chunk.pats.iter().map(convert_pat_info).collect(),
        fmts: chunk.fmts.clone(),
        struct_lits: chunk.struct_lits.iter().map(convert_lit).collect(),
        casts: chunk.casts.iter().map(|t| cast_target(t)).collect(),
        paths: chunk.paths.iter().map(|(segs, _)| segs.clone()).collect(),
        names: chunk.names.clone(),
        children: chunk.children.iter().map(|c| convert(c)).collect(),
        child_caps: chunk.child_caps.clone(),
    })
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
        pat: convert_pat(&info.pat),
        binds: info.binds.clone(),
    }
}

fn convert_pat(pat: &Pat) -> PPat {
    match pat {
        Pat::Wild(_) => PPat::Wild,
        Pat::Rest(_) => PPat::Rest,
        Pat::Ident(id) => PPat::Ident {
            name: id.ident.to_string(),
            sub: id.subpat.as_ref().map(|s| Box::new(convert_pat(&s.1))),
        },
        Pat::Lit(l) => convert_lit_pat(&l.lit),
        Pat::Paren(p) => convert_pat(&p.pat),
        Pat::Reference(r) => convert_pat(&r.pat),
        Pat::Type(t) => convert_pat(&t.pat),
        Pat::Tuple(t) => PPat::Tuple(t.elems.iter().map(convert_pat).collect()),
        Pat::TupleStruct(ts) => PPat::TupleStruct {
            name: ts.path.segments.last().map(|s| s.ident.to_string()),
            elems: ts.elems.iter().map(convert_pat).collect(),
        },
        Pat::Path(p) => PPat::Path {
            name: p.path.segments.last().map(|s| s.ident.to_string()),
        },
        Pat::Struct(s) => PPat::Struct {
            name: s.path.segments.last().map(|s| s.ident.to_string()),
            fields: s
                .fields
                .iter()
                .map(|f| {
                    let key = match &f.member {
                        syn::Member::Named(n) => n.to_string(),
                        syn::Member::Unnamed(i) => i.index.to_string(),
                    };
                    (key, convert_pat(&f.pat))
                })
                .collect(),
        },
        Pat::Or(or) => PPat::Or(or.cases.iter().map(convert_pat).collect()),
        Pat::Slice(s) => PPat::Slice(s.elems.iter().map(convert_pat).collect()),
        _ => PPat::Unsupported,
    }
}

/// Reduce a cast target type to its type name, the only part the VM needs.
fn cast_target(ty: &Type) -> String {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn convert_lit_pat(lit: &Lit) -> PPat {
    match lit {
        Lit::Int(i) => i
            .base10_parse::<i64>()
            .map(|v| PPat::Lit(PLit::Int(v)))
            .unwrap_or(PPat::Unsupported),
        Lit::Float(f) => f
            .base10_parse::<f64>()
            .map(|v| PPat::Lit(PLit::Float(v)))
            .unwrap_or(PPat::Unsupported),
        Lit::Bool(b) => PPat::Lit(PLit::Bool(b.value)),
        Lit::Str(s) => PPat::Lit(PLit::Str(s.value())),
        Lit::Char(c) => PPat::Lit(PLit::Char(c.value())),
        Lit::Byte(b) => PPat::Lit(PLit::Int(b.value() as i64)),
        _ => PPat::Unsupported,
    }
}
