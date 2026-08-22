//! Local type inference, one pass per function body before lowering. The result is one table,
//! the type of every expression, which every consumer in the compiler reads. Nothing is
//! rejected here, `rustc` did that in `rust check`, so an expression this pass cannot type is
//! `Unknown` and the runtime falls back to what the value says.
//!
//! The pass is bidirectional in the small. An annotation or a signature flows down into literals,
//! closures and constructor arguments, and a literal nothing types ends as `i32` or `f64`.

mod exprs;
mod macros;
mod methods;
mod methods_option;
mod methods_scalar;
mod paths;
mod ty;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use syn::{Block, Expr, Lit, Pat, Stmt};

use super::Ctx;
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::Res;

pub(super) use ty::Ty;
use ty::Vars;

/// A macro body parsed once, so the compiler lowers the same nodes this pass typed.
pub(super) enum MacroBody {
    Exprs(Vec<Expr>),
    Repeat(Box<(Expr, Expr)>),
    Matches(Box<(Expr, Pat, Option<Expr>)>),
}

pub(super) struct Types {
    exprs: HashMap<*const Expr, Ty>,
    /// the same types by the address of the inner node, for a consumer holding the
    /// `ExprMethodCall` or `ExprCall` and not the `Expr` around it
    nodes: HashMap<*const (), Ty>,
    macros: HashMap<*const syn::Macro, Rc<MacroBody>>,
}

impl Types {
    pub(super) fn empty() -> Types {
        Types {
            exprs: HashMap::new(),
            nodes: HashMap::new(),
            macros: HashMap::new(),
        }
    }

    pub(super) fn of(&self, expr: &Expr) -> Ty {
        self.exprs
            .get(&std::ptr::from_ref(expr))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    pub(super) fn of_node<T>(&self, node: &T) -> Ty {
        self.nodes
            .get(&std::ptr::from_ref(node).cast::<()>())
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    pub(super) fn macro_body(&self, mac: &syn::Macro) -> Option<Rc<MacroBody>> {
        self.macros.get(&std::ptr::from_ref(mac)).cloned()
    }
}

pub(super) fn infer_fn(ctx: &Ctx, sig: &syn::Signature, block: &Block) -> Types {
    let mut pass = Infer::new(ctx);
    pass.generics = sig
        .generics
        .type_params()
        .map(|p| Arc::from(p.ident.to_string().as_str()))
        .collect();
    pass.ret = match &sig.output {
        syn::ReturnType::Type(_, ty) => pass.lower(ty),
        syn::ReturnType::Default => Ty::Unit,
    };
    pass.push();
    for input in &sig.inputs {
        match input {
            syn::FnArg::Receiver(_) => {
                let self_ty = pass.self_ty();
                pass.define("self", self_ty);
            }
            syn::FnArg::Typed(t) => {
                let ty = pass.lower(&t.ty);
                pass.bind_pat(&t.pat, &ty);
            }
        }
    }
    let ret = pass.ret.clone();
    pass.block_inner(block, &ret);
    pass.finish()
}

pub(super) fn infer_const(ctx: &Ctx, expr: &Expr, ty: Option<&syn::Type>) -> Types {
    let mut pass = Infer::new(ctx);
    pass.push();
    let expected = ty.map_or(Ty::Unknown, |ty| pass.lower(ty));
    pass.expr(expr, &expected);
    pass.finish()
}

struct Infer<'c, 'r> {
    ctx: &'c Ctx<'r>,
    vars: Vars,
    types: HashMap<*const Expr, Ty>,
    nodes: HashMap<*const (), Ty>,
    macros: HashMap<*const syn::Macro, Rc<MacroBody>>,
    scopes: Vec<HashMap<String, Ty>>,
    ret: Ty,
    /// the value type of each enclosing `loop`, for `break v`
    loops: Vec<Ty>,
    generics: Vec<Arc<str>>,
}

impl<'c, 'r> Infer<'c, 'r> {
    fn new(ctx: &'c Ctx<'r>) -> Infer<'c, 'r> {
        Infer {
            ctx,
            vars: Vars::new(),
            types: HashMap::new(),
            nodes: HashMap::new(),
            macros: HashMap::new(),
            scopes: Vec::new(),
            ret: Ty::Unit,
            loops: Vec::new(),
            generics: Vec::new(),
        }
    }

    fn finish(self) -> Types {
        let exprs = self
            .types
            .iter()
            .map(|(ptr, ty)| (*ptr, self.vars.resolve(ty)))
            .collect();
        let nodes = self
            .nodes
            .iter()
            .map(|(ptr, ty)| (*ptr, self.vars.resolve(ty)))
            .collect();
        Types {
            exprs,
            nodes,
            macros: self.macros,
        }
    }

    // scopes

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn self_ty(&self) -> Ty {
        match self.ctx.impl_type {
            Some(ty) => self.user_type(ty),
            None => Ty::Unknown,
        }
    }

    /// A declared struct or enum by canonical name, or a bridge type by its bare name.
    fn user_type(&self, canon: &str) -> Ty {
        if self.ctx.resolver.structs.contains_key(canon) {
            Ty::Struct(Arc::from(canon))
        } else if self.ctx.resolver.enums.contains_key(canon) {
            Ty::Enum(Arc::from(canon))
        } else {
            Ty::named(canon)
        }
    }

    // types written in the source

    fn lower(&self, ty: &syn::Type) -> Ty {
        self.lower_in(ty, self.ctx.module)
    }

    fn lower_in(&self, ty: &syn::Type, module: usize) -> Ty {
        match ty {
            syn::Type::Paren(p) => self.lower_in(&p.elem, module),
            syn::Type::Group(g) => self.lower_in(&g.elem, module),
            syn::Type::Reference(r) => self.lower_in(&r.elem, module),
            syn::Type::Tuple(t) if t.elems.is_empty() => Ty::Unit,
            syn::Type::Tuple(t) => {
                Ty::Tuple(t.elems.iter().map(|e| self.lower_in(e, module)).collect())
            }
            syn::Type::Array(a) => Ty::vec(self.lower_in(&a.elem, module)),
            syn::Type::Slice(s) => Ty::vec(self.lower_in(&s.elem, module)),
            syn::Type::Path(p) => self.lower_path(p, module),
            _ => Ty::Unknown,
        }
    }

    fn lower_path(&self, p: &syn::TypePath, module: usize) -> Ty {
        if let Some(qself) = &p.qself {
            return self.lower_in(&qself.ty, module);
        }
        let Some(last) = p.path.segments.last() else {
            return Ty::Unknown;
        };
        let name = last.ident.to_string();
        let arg = |i: usize| type_arg(last, i).map_or(Ty::Unknown, |t| self.lower_in(t, module));
        if p.path.segments.len() == 1
            && matches!(last.arguments, syn::PathArguments::None)
            && self.generics.iter().any(|g| **g == *name)
        {
            return Ty::Generic(Arc::from(name.as_str()));
        }
        if let Some(width) = IntWidth::parse(&name) {
            return Ty::Int(width);
        }
        match name.as_str() {
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "char" => Ty::Char,
            "String" | "str" | "OsString" | "OsStr" => Ty::Str,
            "Vec" | "VecDeque" => Ty::vec(arg(0)),
            "HashSet" | "BTreeSet" => Ty::Set(Box::new(arg(0))),
            "HashMap" | "BTreeMap" | "IndexMap" => Ty::Map(Box::new(arg(0)), Box::new(arg(1))),
            "Option" => Ty::option(arg(0)),
            "Result" => {
                let err = if type_arg(last, 1).is_some() {
                    arg(1)
                } else {
                    Ty::named("anyhow::Error")
                };
                Ty::result(arg(0), err)
            }
            "Box" | "Rc" | "Arc" | "RefCell" | "Cell" | "Mutex" | "Cow" => arg(0),
            "Value" if p.path.segments.len() == 1 || segment_is(&p.path, "serde_json") => Ty::Json,
            "Self" => self.self_ty(),
            _ => {
                let segs: Vec<String> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                match self.ctx.resolver.resolve(module, &segs) {
                    Ok(Res::Struct(canon)) => Ty::Struct(canon),
                    Ok(Res::Enum(canon)) => Ty::Enum(canon),
                    Ok(Res::Alias(m, target)) => self.lower_in(&target, m),
                    _ => Ty::Named(
                        Arc::from(name.as_str()),
                        (0..2)
                            .filter_map(|i| type_arg(last, i))
                            .map(|t| self.lower_in(t, module))
                            .collect(),
                    ),
                }
            }
        }
    }

    /// The declared type of a struct field, lowered in the struct's own module.
    fn field_ty(&self, canon: &str, member: &syn::Member) -> Ty {
        let Some(def) = self.ctx.resolver.structs.get(canon) else {
            return Ty::Unknown;
        };
        let field = match (&def.ast.fields, member) {
            (syn::Fields::Named(named), syn::Member::Named(name)) => named
                .named
                .iter()
                .find(|f| f.ident.as_ref().is_some_and(|i| i == name)),
            (syn::Fields::Unnamed(unnamed), syn::Member::Unnamed(index)) => {
                unnamed.unnamed.iter().nth(index.index as usize)
            }
            _ => None,
        };
        field.map_or(Ty::Unknown, |f| self.lower_in(&f.ty, def.module))
    }

    /// The payload types of an enum variant in declaration order, named fields with their name.
    fn variant_payload(&self, canon: &str, variant: &str) -> Vec<(Option<String>, Ty)> {
        let Some(def) = self.ctx.resolver.enums.get(canon) else {
            return Vec::new();
        };
        let Some(v) = def.variants.iter().find(|v| v.ident == variant) else {
            return Vec::new();
        };
        v.fields
            .iter()
            .map(|f| {
                (
                    f.ident.as_ref().map(ToString::to_string),
                    self.lower_in(&f.ty, self.ctx.module),
                )
            })
            .collect()
    }

    // blocks and statements

    fn block(&mut self, block: &Block, expected: &Ty) -> Ty {
        self.push();
        let ty = self.block_inner(block, expected);
        self.pop();
        ty
    }

    fn block_inner(&mut self, block: &Block, expected: &Ty) -> Ty {
        // items bind first, like hoisting
        for stmt in &block.stmts {
            if let Stmt::Item(syn::Item::Const(c)) = stmt {
                let ty = self.lower(&c.ty);
                self.expr(&c.expr, &ty);
                self.define(&c.ident.to_string(), ty);
            }
            if let Stmt::Item(syn::Item::Static(s)) = stmt {
                let ty = self.lower(&s.ty);
                self.expr(&s.expr, &ty);
                self.define(&s.ident.to_string(), ty);
            }
        }
        let last = block.stmts.len().saturating_sub(1);
        let mut out = Ty::Unit;
        for (i, stmt) in block.stmts.iter().enumerate() {
            match stmt {
                Stmt::Local(local) => self.local(local),
                Stmt::Expr(e, semi) => {
                    if i == last && semi.is_none() {
                        out = self.expr(e, expected);
                    } else {
                        self.expr(e, &Ty::Unknown);
                    }
                }
                Stmt::Macro(m) => {
                    let ty = self.macro_ty(&m.mac, expected);
                    if i == last && m.semi_token.is_none() {
                        out = ty;
                    }
                }
                Stmt::Item(_) => {}
            }
        }
        if block.stmts.is_empty() {
            return Ty::Unit;
        }
        out
    }

    fn local(&mut self, local: &syn::Local) {
        let (pat, annotation) = match &local.pat {
            Pat::Type(t) => (&*t.pat, self.lower(&t.ty)),
            other => (other, Ty::Unknown),
        };
        let ty = match &local.init {
            Some(init) => {
                let found = self.expr(&init.expr, &annotation);
                if let Some((_, diverge)) = &init.diverge {
                    self.expr(diverge, &Ty::Unknown);
                }
                self.vars.meet(&found, &annotation)
            }
            None => annotation,
        };
        self.bind_pat(pat, &ty);
    }

    // patterns

    fn bind_pat(&mut self, pat: &Pat, ty: &Ty) {
        match pat {
            Pat::Ident(id) => {
                if let Some((_, sub)) = &id.subpat {
                    self.bind_pat(sub, ty);
                }
                if !super::pattern::is_unit_variant_ident(id) {
                    self.define(&id.ident.to_string(), ty.clone());
                }
            }
            Pat::Type(t) => {
                let declared = self.lower(&t.ty);
                let ty = self.vars.meet(ty, &declared);
                self.bind_pat(&t.pat, &ty);
            }
            Pat::Paren(p) => self.bind_pat(&p.pat, ty),
            Pat::Reference(r) => self.bind_pat(&r.pat, ty),
            Pat::Guard(g) => {
                self.bind_pat(&g.pat, ty);
                self.expr(&g.guard, &Ty::Bool);
            }
            Pat::Tuple(t) => {
                let items = match ty {
                    Ty::Tuple(items) => items.clone(),
                    _ => Vec::new(),
                };
                for (i, p) in t.elems.iter().enumerate() {
                    let item = items.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_pat(p, &item);
                }
            }
            Pat::Slice(s) => {
                let item = ty.item();
                for p in &s.elems {
                    match p {
                        Pat::Rest(_) => {}
                        Pat::Ident(id)
                            if id
                                .subpat
                                .as_ref()
                                .is_some_and(|(_, sub)| matches!(**sub, Pat::Rest(_))) =>
                        {
                            self.define(&id.ident.to_string(), ty.clone());
                        }
                        other => self.bind_pat(other, &item),
                    }
                }
            }
            Pat::TupleStruct(ts) => {
                let payloads = self.variant_fields(&ts.path, ty);
                for (i, p) in ts.elems.iter().enumerate() {
                    let item = payloads.get(i).map_or(Ty::Unknown, |(_, t)| t.clone());
                    self.bind_pat(p, &item);
                }
            }
            Pat::Struct(s) => {
                let canon = self.struct_key(&s.path);
                for field in &s.fields {
                    let fty = match (&canon, ty) {
                        (Some(canon), _) => self.field_ty(canon, &field.member),
                        (None, Ty::Enum(enum_canon)) => {
                            let variant = s.path.segments.last().map(|s| s.ident.to_string());
                            let name = match &field.member {
                                syn::Member::Named(n) => n.to_string(),
                                syn::Member::Unnamed(i) => i.index.to_string(),
                            };
                            variant
                                .and_then(|v| {
                                    self.variant_payload(enum_canon, &v)
                                        .into_iter()
                                        .find(|(n, _)| n.as_deref() == Some(name.as_str()))
                                })
                                .map_or(Ty::Unknown, |(_, t)| t)
                        }
                        _ => Ty::Unknown,
                    };
                    self.bind_pat(&field.pat, &fty);
                }
            }
            Pat::Or(o) => {
                for case in &o.cases {
                    self.bind_pat(case, ty);
                }
            }
            Pat::Lit(l) => {
                let lit = Expr::Lit(l.clone());
                self.literal(&lit.clone(), ty);
            }
            _ => {}
        }
    }

    /// `Some(x)`, `Ok(x)`, `Err(e)` and a user variant, in payload order.
    fn variant_fields(&self, path: &syn::Path, scrutinee: &Ty) -> Vec<(Option<String>, Ty)> {
        let last = path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        match (last.as_str(), scrutinee) {
            ("Some", Ty::Option(t)) | ("Ok", Ty::Result(t, _)) => vec![(None, (**t).clone())],
            ("Err", Ty::Result(_, e)) => vec![(None, (**e).clone())],
            (variant, Ty::Enum(canon)) => self.variant_payload(canon, variant),
            (_, Ty::Struct(canon)) => {
                let Some(def) = self.ctx.resolver.structs.get(&**canon) else {
                    return Vec::new();
                };
                let module = def.module;
                def.ast
                    .fields
                    .iter()
                    .map(|f| (None, self.lower_in(&f.ty, module)))
                    .collect()
            }
            _ => {
                // a path that names the enum itself
                let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
                if let Ok(Res::TypeMember(canon, rest)) =
                    self.ctx.resolver.resolve(self.ctx.module, &segs)
                    && let [variant] = rest.as_slice()
                {
                    return self.variant_payload(&canon, variant);
                }
                if let Ok(Res::Struct(canon)) = self.ctx.resolver.resolve(self.ctx.module, &segs)
                    && let Some(def) = self.ctx.resolver.structs.get(&canon)
                {
                    let module = def.module;
                    return def
                        .ast
                        .fields
                        .iter()
                        .map(|f| (None, self.lower_in(&f.ty, module)))
                        .collect();
                }
                Vec::new()
            }
        }
    }

    fn struct_key(&self, path: &syn::Path) -> Option<Arc<str>> {
        if path.segments.len() == 1 && path.segments[0].ident == "Self" {
            return self.ctx.impl_type.map(Arc::from);
        }
        self.ctx.resolver.resolve_struct_key(self.ctx.module, path)
    }

    // expressions
}

fn lit_ty(lit: &Lit) -> Ty {
    match lit {
        Lit::Str(_) => Ty::Str,
        Lit::ByteStr(_) => Ty::vec(Ty::Int(IntWidth::U8)),
        Lit::Byte(_) => Ty::Int(IntWidth::U8),
        Lit::Char(_) => Ty::Char,
        Lit::Bool(_) => Ty::Bool,
        _ => Ty::Unknown,
    }
}

pub(super) fn type_arg(seg: &syn::PathSegment, i: usize) -> Option<&syn::Type> {
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|g| match g {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .nth(i),
        _ => None,
    }
}

fn segment_is(path: &syn::Path, name: &str) -> bool {
    path.segments.iter().any(|s| s.ident == name)
}
