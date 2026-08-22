//! User declared types. A `UserShape` is what typing needs and travels
//! inside `Ty`. A `UserDef` is the shape plus every body.

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
use crate::lang::fmt::FmtSpec;
use crate::lang::ty::Ty;

/// `Ord` implies `Eq`, so the 2 are one axis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Compare {
    #[default]
    Partial,
    Eq,
    Ord,
}

/// `Debug`, `Clone` and `PartialEq` are always derived.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Derives {
    pub compare: Compare,
    pub hash: bool,
    pub default: bool,
}

impl Derives {
    pub fn is_eq(self) -> bool {
        self.compare != Compare::Partial
    }

    pub fn is_ord(self) -> bool {
        self.compare == Compare::Ord
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
}

/// Unit when the payload is empty.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub payload: Vec<Ty>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum UserKind {
    Struct(Vec<Field>),
    Enum(Vec<Variant>),
}

/// `Self` is spelled apart so a shape never contains itself.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Ret {
    Same,
    Ty(Ty),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum MethodKind {
    /// `fn name(&self, ..)`.
    Method,
    /// `fn name(..) -> Self`, called as `Type::name(..)`.
    Assoc,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct MethodSig {
    pub name: String,
    pub kind: MethodKind,
    pub args: Vec<Ty>,
    pub ret: Ret,
}

impl MethodSig {
    pub fn ret_ty(&self, owner: &Ty) -> Ty {
        match &self.ret {
            Ret::Same => owner.clone(),
            Ret::Ty(ty) => ty.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct UserShape {
    pub name: String,
    pub kind: UserKind,
    pub derives: Derives,
    pub display: bool,
    /// Implements the program local `DiffDescribe` trait.
    pub describe: bool,
    pub methods: Vec<MethodSig>,
    /// Source types of the `From` impls.
    pub froms: Vec<Ty>,
    pub depth: usize,
    pub has_float: bool,
}

impl UserShape {
    pub fn is_enum(&self) -> bool {
        matches!(self.kind, UserKind::Enum(_))
    }

    pub fn fields(&self) -> &[Field] {
        match &self.kind {
            UserKind::Struct(fields) => fields,
            UserKind::Enum(_) => &[],
        }
    }

    pub fn variants(&self) -> &[Variant] {
        match &self.kind {
            UserKind::Enum(variants) => variants,
            UserKind::Struct(_) => &[],
        }
    }

    pub fn converts_from(&self, from: &Ty) -> bool {
        self.froms.contains(from)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DisplayForm {
    /// `write!(f, "..", fields..)`, ignores the caller's width like most
    /// hand written impls.
    Write,
    /// `f.pad(&format!(..))`, which honors them.
    Pad,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayPiece {
    pub spec: FmtSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayImpl {
    pub form: DisplayForm,
    /// Per struct field, or per enum variant a list per payload slot.
    pub pieces: Vec<Vec<DisplayPiece>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserMethod {
    pub sig: MethodSig,
    pub params: Vec<String>,
    pub body: Expr,
}

/// A struct receives the value in one field and defaults the rest, an enum
/// wraps it in one variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FromImpl {
    pub src: Ty,
    pub slot: usize,
    /// The fields a struct fills with literals when it cannot default.
    pub rest: Vec<Expr>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserDef {
    pub shape: UserShape,
    pub display: Option<DisplayImpl>,
    pub methods: Vec<UserMethod>,
    pub froms: Vec<FromImpl>,
}

impl UserDef {
    pub fn ty(&self) -> Ty {
        Ty::user(self.shape.clone())
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.render_decl());
        if let Some(display) = &self.display {
            out.push_str(&self.render_display(display));
        }
        if !self.methods.is_empty() {
            out.push_str(&self.render_methods());
        }
        for from in &self.froms {
            out.push_str(&self.render_from(from));
        }
        if self.shape.describe {
            out.push_str(&format!(
                "impl DiffDescribe for {} {{\n    fn diff_describe(&self) -> String {{\n        format!(\"{}={{:?}}\", self)\n    }}\n}}\n\n",
                self.shape.name, self.shape.name
            ));
        }
        out
    }

    fn derive_line(&self) -> String {
        let mut names = vec!["Debug", "Clone", "PartialEq"];
        let derives = self.shape.derives;
        if derives.is_eq() {
            names.push("Eq");
        }
        if derives.hash {
            names.push("Hash");
        }
        if derives.is_ord() {
            names.push("PartialOrd");
            names.push("Ord");
        }
        if derives.default {
            names.push("Default");
        }
        format!("#[derive({})]\n", names.join(", "))
    }

    fn render_decl(&self) -> String {
        let mut out = self.derive_line();
        match &self.shape.kind {
            UserKind::Struct(fields) => {
                out.push_str(&format!("struct {} {{\n", self.shape.name));
                for field in fields {
                    out.push_str(&format!("    {}: {},\n", field.name, field.ty.rust()));
                }
                out.push_str("}\n\n");
            }
            UserKind::Enum(variants) => {
                out.push_str(&format!("enum {} {{\n", self.shape.name));
                for (index, variant) in variants.iter().enumerate() {
                    // The first unit variant carries the derived `Default`.
                    if index == 0 && self.shape.derives.default {
                        out.push_str("    #[default]\n");
                    }
                    if variant.payload.is_empty() {
                        out.push_str(&format!("    {},\n", variant.name));
                    } else {
                        let payload: Vec<String> = variant.payload.iter().map(Ty::rust).collect();
                        out.push_str(&format!("    {}({}),\n", variant.name, payload.join(", ")));
                    }
                }
                out.push_str("}\n\n");
            }
        }
        out
    }

    fn render_display(&self, display: &DisplayImpl) -> String {
        let mut out = format!(
            "impl std::fmt::Display for {} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n",
            self.shape.name
        );
        match &self.shape.kind {
            UserKind::Struct(fields) => {
                let pieces = display.pieces.first().map_or(&[][..], Vec::as_slice);
                let mut template = self.shape.name.clone();
                let mut args = Vec::new();
                for (field, piece) in fields.iter().zip(pieces) {
                    template.push_str(&format!(" {}={}", field.name, piece.spec.placeholder()));
                    args.push(format!("self.{}", field.name));
                }
                out.push_str(&render_write(&display.form, &template, &args, 2));
            }
            UserKind::Enum(variants) => {
                out.push_str("        match self {\n");
                for (variant, pieces) in variants.iter().zip(&display.pieces) {
                    let binds: Vec<String> = (0..variant.payload.len())
                        .map(|slot| format!("diff_p{slot}"))
                        .collect();
                    let pattern = if binds.is_empty() {
                        format!("Self::{}", variant.name)
                    } else {
                        format!("Self::{}({})", variant.name, binds.join(", "))
                    };
                    let mut template = variant.name.clone();
                    for piece in pieces {
                        template.push_str(&format!(" {}", piece.spec.placeholder()));
                    }
                    out.push_str(&format!("            {pattern} => "));
                    out.push_str(render_write(&display.form, &template, &binds, 0).trim_end());
                    out.push_str(",\n");
                }
                out.push_str("        }\n");
            }
        }
        out.push_str("    }\n}\n\n");
        out
    }

    fn render_methods(&self) -> String {
        let mut out = format!("impl {} {{\n", self.shape.name);
        for method in &self.methods {
            let mut params = Vec::new();
            if method.sig.kind == MethodKind::Method {
                params.push("&self".to_string());
            }
            for (name, ty) in method.params.iter().zip(&method.sig.args) {
                params.push(format!("{name}: {}", ty.rust()));
            }
            let ret = match &method.sig.ret {
                Ret::Same => "Self".to_string(),
                Ret::Ty(ty) => ty.rust(),
            };
            out.push_str(&format!(
                "    fn {}({}) -> {ret} {{\n        {}\n    }}\n",
                method.sig.name,
                params.join(", "),
                method.body.render()
            ));
        }
        out.push_str("}\n\n");
        out
    }

    fn render_from(&self, from: &FromImpl) -> String {
        let body = match &self.shape.kind {
            UserKind::Struct(fields) => {
                let mut inits = Vec::new();
                let mut rest = from.rest.iter();
                for (index, field) in fields.iter().enumerate() {
                    if index == from.slot {
                        inits.push(format!("{}: value", field.name));
                    } else if let Some(expr) = rest.next() {
                        inits.push(format!("{}: {}", field.name, expr.render()));
                    }
                }
                if fields.len() > 1 && from.rest.is_empty() {
                    inits.push("..Default::default()".to_string());
                }
                format!("Self {{ {} }}", inits.join(", "))
            }
            UserKind::Enum(variants) => {
                let variant = &variants[from.slot];
                if variant.payload.len() == 1 {
                    format!("Self::{}(value)", variant.name)
                } else {
                    let mut args = vec!["value".to_string()];
                    args.extend(from.rest.iter().map(Expr::render));
                    format!("Self::{}({})", variant.name, args.join(", "))
                }
            }
        };
        format!(
            "impl From<{}> for {} {{\n    fn from(value: {}) -> Self {{\n        {body}\n    }}\n}}\n\n",
            from.src.rust(),
            self.shape.name,
            from.src.rust()
        )
    }
}

fn render_write(form: &DisplayForm, template: &str, args: &[String], indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let args_text = if args.is_empty() {
        String::new()
    } else {
        format!(", {}", args.join(", "))
    };
    match form {
        DisplayForm::Write => format!("{pad}write!(f, \"{template}\"{args_text})\n"),
        DisplayForm::Pad => format!("{pad}f.pad(&format!(\"{template}\"{args_text}))\n"),
    }
}

/// One declaration per program.
pub const DESCRIBE_TRAIT: &str =
    "trait DiffDescribe {\n    fn diff_describe(&self) -> String;\n}\n\n";

/// `impl DiffDescribe for <builtin>`.
pub fn render_describe_impl(ty: &Ty) -> String {
    format!(
        "impl DiffDescribe for {} {{\n    fn diff_describe(&self) -> String {{\n        format!(\"{}={{:?}}\", self)\n    }}\n}}\n\n",
        ty.rust(),
        ty.rust().replace('<', "[").replace('>', "]")
    )
}
