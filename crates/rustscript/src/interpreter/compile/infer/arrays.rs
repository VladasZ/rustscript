//! The array lengths a `try_into` lands in. The type inference lowers an array to a vec, so the
//! length a slice must have is kept by call, from the annotation or argument around it.

use syn::Expr;

use super::Infer;
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    fn note_array_target(&mut self, init: &Expr, len: usize) {
        if let Some(m) = super::super::walks::try_into_root(init) {
            self.array_lens.insert(std::ptr::from_ref(m).cast(), len);
        }
    }

    /// `let word: [u8; 4] = bytes[..4].try_into()?`
    pub(super) fn note_let_array(&mut self, local: &syn::Local) {
        if let syn::Pat::Type(t) = &local.pat
            && let Some(init) = &local.init
            && let Some(len) = array_len_of(&t.ty)
        {
            self.note_array_target(&init.expr, len);
        }
    }

    /// `u32::from_be_bytes(b[..4].try_into().unwrap())` takes an array of the type's width.
    pub(super) fn note_bytes_call(&mut self, segs: &[String], args: &[&Expr]) {
        let [owner, f] = segs else { return };
        if !matches!(
            f.as_str(),
            "from_be_bytes" | "from_le_bytes" | "from_ne_bytes"
        ) {
            return;
        }
        let width = match owner.as_str() {
            "f32" => Some(4),
            "f64" => Some(8),
            other => IntWidth::parse(other).map(|w| w.bits() as usize / 8),
        };
        if let (Some(len), Some(first)) = (width, args.first()) {
            self.note_array_target(first, len);
        }
    }

    /// A script fn param written `[u8; 4]`.
    pub(super) fn note_param_arrays(&mut self, sig: &syn::Signature, args: &[&Expr]) {
        let typed = sig.inputs.iter().filter_map(|input| match input {
            syn::FnArg::Receiver(_) => None,
            syn::FnArg::Typed(t) => Some(&*t.ty),
        });
        for (ty, arg) in typed.zip(args) {
            if let Some(len) = array_len_of(ty) {
                self.note_array_target(arg, len);
            }
        }
    }
}

/// `[u8; 4]` with its length written as a literal, or the `Ok` of a `Result<[u8; 4], _>` that a
/// bare `try_into` lands in. A named const length is not read.
fn array_len_of(ty: &syn::Type) -> Option<usize> {
    match ty {
        syn::Type::Array(array) => {
            let Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(n),
                ..
            }) = &array.len
            else {
                return None;
            };
            n.base10_parse().ok()
        }
        syn::Type::Path(p) => {
            let seg = p.path.segments.last()?;
            if seg.ident != "Result" {
                return None;
            }
            match super::super::first_generic_type(seg)? {
                inner @ syn::Type::Array(_) => array_len_of(inner),
                _ => None,
            }
        }
        _ => None,
    }
}
