//! `Default` lowering and the return type hints of a function.

use std::sync::Arc;

use syn::{Block, Expr};

use super::walks::returned_exprs;
use super::{Compiler, derives_default, idx16};
use crate::interpreter::bytecode::{DefaultIr, EnumVariant, NO_CONV, Op, Reg, StructShape};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::Res;

/// So a struct holding a `Vec` of itself still terminates.
pub(super) const DEFAULT_DEPTH: usize = 8;

impl Compiler<'_> {
    /// `None` when the type has no `Default` this interpreter can build.
    pub(super) fn default_ir(&mut self, ty: &syn::Type) -> Option<DefaultIr> {
        self.default_ir_at(ty, 0)
    }

    pub(super) fn default_ir_at(&mut self, ty: &syn::Type, depth: usize) -> Option<DefaultIr> {
        if depth > DEFAULT_DEPTH {
            return None;
        }
        match ty {
            syn::Type::Tuple(t) if t.elems.is_empty() => Some(DefaultIr::Unit),
            syn::Type::Tuple(t) => {
                let items = t
                    .elems
                    .iter()
                    .map(|elem| self.default_ir_at(elem, depth + 1))
                    .collect::<Option<Vec<_>>>()?;
                Some(DefaultIr::Tuple(items))
            }
            syn::Type::Paren(p) => self.default_ir_at(&p.elem, depth),
            syn::Type::Group(g) => self.default_ir_at(&g.elem, depth),
            syn::Type::Path(p) => self.default_ir_path(&p.path, depth),
            _ => None,
        }
    }

    pub(super) fn default_ir_path(&mut self, path: &syn::Path, depth: usize) -> Option<DefaultIr> {
        let last = path.segments.last()?.ident.to_string();
        if let Some(width) = IntWidth::parse(&last) {
            return Some(DefaultIr::Int(width));
        }
        let builtin = match last.as_str() {
            "f32" => Some(DefaultIr::F32),
            "f64" => Some(DefaultIr::F64),
            "bool" => Some(DefaultIr::Bool),
            "char" => Some(DefaultIr::Char),
            "String" | "str" => Some(DefaultIr::Str),
            "Vec" | "VecDeque" => Some(DefaultIr::Vec),
            "HashMap" | "BTreeMap" => Some(DefaultIr::Map),
            "HashSet" | "BTreeSet" => Some(DefaultIr::Set),
            "Option" => Some(DefaultIr::Opt),
            _ => None,
        };
        if builtin.is_some() {
            return builtin;
        }
        let mut segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs.len() == 1
            && segs[0] == "Self"
            && let Some(ty) = self.ctx.impl_type
        {
            segs[0] = ty.to_string();
        }
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) => self.default_ir_struct(&canon, depth),
            Res::Enum(canon) => self.default_ir_enum(&canon),
            Res::Alias(m, target) => match &*target {
                syn::Type::Path(p) => {
                    let canon = self.ctx.resolver.resolve_struct_key(m, &p.path)?;
                    self.default_ir_struct(&canon, depth)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// 1 default per field in declaration order
    pub(super) fn default_ir_struct(
        &mut self,
        canon: &Arc<str>,
        depth: usize,
    ) -> Option<DefaultIr> {
        let def = self.ctx.resolver.structs.get(canon)?;
        if !derives_default(&def.ast.attrs) {
            return None;
        }
        let ast = def.ast.clone();
        let mut names = Vec::new();
        let mut renames = Vec::new();
        let mut fields = Vec::new();
        for field in &ast.fields {
            let name = field.ident.as_ref()?.to_string();
            names.push(Arc::<str>::from(name));
            renames
                .push(crate::interpreter::serde_attrs::serde_rename(field).map(Arc::<str>::from));
            fields.push(self.default_ir_at(&field.ty, depth + 1)?);
        }
        let shape = self.shape_for(canon, names, renames);
        Some(DefaultIr::Struct { shape, fields })
    }

    pub(super) fn default_ir_enum(&mut self, canon: &Arc<str>) -> Option<DefaultIr> {
        let ast = self.ctx.resolver.enums.get(canon)?.clone();
        if !derives_default(&ast.attrs) {
            return None;
        }
        let variant = ast.variants.iter().find(|variant| {
            variant
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("default"))
        })?;
        let name = variant.ident.to_string();
        let def = self.ctx.resolver.enum_defs.get(canon)?;
        Some(DefaultIr::Enum(EnumVariant {
            def: def.clone(),
            variant: def.variant_index(&name)?,
        }))
    }

    /// Shared with any literal of the same layout.
    pub(super) fn shape_for(
        &mut self,
        name: &Arc<str>,
        fields: Vec<Arc<str>>,
        renames: Vec<Option<Arc<str>>>,
    ) -> Arc<StructShape> {
        if let Some(known) = self
            .shapes
            .iter()
            .find(|s| s.name == *name && s.fields == fields && s.renames == renames)
        {
            return known.clone();
        }
        let type_id = self.ctx.resolver.type_id_of(name);
        let built = StructShape::typed(name.clone(), type_id, fields, renames);
        self.shapes.push(built.clone());
        built
    }

    pub(super) fn default_ir_for_struct(&mut self, canon: &Arc<str>) -> Option<DefaultIr> {
        self.default_ir_struct(canon, 0)
    }

    pub(super) fn default_ir_path_pub(&mut self, path: &syn::Path) -> Option<DefaultIr> {
        self.default_ir_path(path, 0)
    }

    /// The error type a `?` converts into, and the type of a bare `Default::default()` handed back.
    pub(super) fn install_return_hints(&mut self, output: &syn::ReturnType, block: &Block) {
        let syn::ReturnType::Type(_, ty) = output else {
            return;
        };
        if let Some(canon) = self.result_error_type(ty) {
            self.cur().ret_error = Some(canon);
        }
        let calls: Vec<*const syn::ExprCall> = returned_exprs(block)
            .into_iter()
            .filter_map(|e| super::struct_lit::bare_default_call(e).map(std::ptr::from_ref))
            .collect();
        if !calls.is_empty()
            && let Some(ir) = self.default_ir(ty)
        {
            for call in calls {
                self.default_calls.insert(call, ir.clone());
            }
        }
        let reductions = returned_exprs(block).into_iter().filter_map(|e| match e {
            Expr::MethodCall(m)
                if m.turbofish.is_none()
                    && matches!(
                        m.method.to_string().as_str(),
                        "sum" | "product" | "unwrap_or_default"
                    ) =>
            {
                Some((std::ptr::from_ref(m), (**ty).clone()))
            }
            _ => None,
        });
        self.return_tails.extend(reductions);
    }

    /// The `E` of a written `Result<T, E>` when it is a script type.
    pub(super) fn result_error_type(&self, ty: &syn::Type) -> Option<Arc<str>> {
        let syn::Type::Path(p) = ty else { return None };
        let last = p.path.segments.last()?;
        if last.ident != "Result" {
            return None;
        }
        let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
            return None;
        };
        let err = args
            .args
            .iter()
            .filter_map(|arg| match arg {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .nth(1)?;
        let syn::Type::Path(err_path) = err else {
            return None;
        };
        let segs: Vec<String> = err_path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) | Res::Enum(canon) => Some(canon),
            _ => None,
        }
    }

    pub(super) fn user_type_key(&self, path: &syn::Path) -> Option<Arc<str>> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolve_path_res(&segs).ok()? {
            Res::Struct(canon) | Res::Enum(canon) => Some(canon),
            _ => None,
        }
    }

    /// The `conv` operand of a `?`, `NO_CONV` without a user error type.
    pub(super) fn try_conv(&mut self) -> u16 {
        let Some(target) = self.cur().ret_error.clone() else {
            return NO_CONV;
        };
        let table = &mut self.cur().try_targets;
        if let Some(index) = table.iter().position(|known| *known == target) {
            return idx16(index);
        }
        table.push(target);
        idx16(table.len() - 1)
    }

    pub(super) fn emit_default(&mut self, dst: Reg, ir: DefaultIr) {
        let table = &mut self.cur().defaults;
        table.push(ir);
        let index = idx16(table.len() - 1);
        self.emit(Op::BuildDefault { dst, ir: index });
    }
}
