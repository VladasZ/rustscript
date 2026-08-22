//! Every node carries its result type, so the generator, renderer and
//! shrinker agree without re-running inference.

use serde::{Deserialize, Serialize};

use crate::lang::catalog::{METHODS, Method};
use crate::lang::pat::Pat;
use crate::lang::pipe::Pipe;
use crate::lang::stmt::Stmt;
use crate::lang::ty::{FloatWidth, IntWidth, StdErr, Ty};
use crate::lang::user::{MethodKind, UserShape};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn token(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::And => "&&",
            Self::Or => "||",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// Whether the operator can abort at runtime. Literals are laundered
    /// before they reach one, so the abort stays a runtime event.
    pub fn is_fallible(self) -> bool {
        matches!(
            self,
            Self::Add | Self::Sub | Self::Mul | Self::Div | Self::Rem | Self::Shl | Self::Shr
        )
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }

    pub fn has_compound(self) -> bool {
        !self.is_comparison() && !matches!(self, Self::And | Self::Or)
    }

    pub fn feature(self) -> &'static str {
        match self {
            Self::Add => "lang-op-add",
            Self::Sub => "lang-op-sub",
            Self::Mul => "lang-op-mul",
            Self::Div => "lang-op-div",
            Self::Rem => "lang-op-rem",
            Self::BitAnd => "lang-op-bitand",
            Self::BitOr => "lang-op-bitor",
            Self::BitXor => "lang-op-bitxor",
            Self::Shl => "lang-op-shl",
            Self::Shr => "lang-op-shr",
            Self::And => "lang-op-and",
            Self::Or => "lang-op-or",
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => "lang-op-compare",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    fn token(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// `opaque` routes it through a helper so the compiler cannot fold it.
    IntLit {
        width: IntWidth,
        value: i128,
        opaque: bool,
    },
    /// Unsuffixed, so `i32` by default. `opaque` wraps it in the
    /// `if diff_opaque_true()` shield, which hides it from the overflow lint
    /// without naming a type.
    BareInt {
        value: i128,
        opaque: bool,
    },
    /// Laundered too, `f32::NEG_INFINITY as u16` is an integer constant the
    /// lints fold.
    FloatLit {
        width: FloatWidth,
        token: String,
        opaque: bool,
    },
    /// Unsuffixed, `f64` by default.
    BareFloat {
        token: String,
        opaque: bool,
    },
    /// `opaque` keeps `rustc` from folding a shift by `'7' as u32`.
    BoolLit {
        value: bool,
        opaque: bool,
    },
    CharLit {
        value: char,
        opaque: bool,
    },
    StrLit(String),
    VecLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    OptLit {
        elem: Ty,
        value: Option<Box<Expr>>,
    },
    /// Built through `insert`. Empty renders as a turbofished `new`.
    MapLit {
        key: Ty,
        value: Ty,
        items: Vec<(Expr, Expr)>,
    },
    SetLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    TupleLit(Vec<Expr>),
    /// `Ok::<T, E>(..)` or `Err::<T, E>(..)`, both types pinned.
    ResLit {
        ok: Ty,
        err: Ty,
        value: Result<Box<Expr>, Box<Expr>>,
    },
    /// Made by a parse that fails.
    StdErrLit(StdErr),
    /// `Name { a: .., b: .. }`, or with `update` the first `fields.len()`
    /// fields written and `..Default::default()` for the rest.
    StructLit {
        shape: Box<UserShape>,
        fields: Vec<Expr>,
        update: bool,
    },
    EnumLit {
        shape: Box<UserShape>,
        variant: usize,
        payload: Vec<Expr>,
    },
    /// `<T>::default()`.
    DefaultOf(Ty),
    /// An iterator pipeline.
    Pipe(Box<Pipe>),
    /// `by_ref` marks the arguments the function takes by `&`.
    FnCall {
        name: String,
        args: Vec<Expr>,
        #[serde(default)]
        by_ref: Vec<bool>,
        ty: Ty,
    },
    ClosureCall {
        name: String,
        args: Vec<Expr>,
        ty: Ty,
    },
    Var {
        name: String,
        ty: Ty,
    },
    /// `opaque` shields it from the overflow lint like a bare literal.
    ConstRef {
        name: String,
        ty: Ty,
        opaque: bool,
    },
    Bin {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        ty: Ty,
    },
    Unary {
        op: UnOp,
        value: Box<Expr>,
        ty: Ty,
    },
    Cast {
        value: Box<Expr>,
        to: Ty,
    },
    /// `method` is the catalog key, the template comes from the catalog.
    Call {
        method: String,
        recv: Box<Expr>,
        args: Vec<Expr>,
        fish: Option<Ty>,
        ty: Ty,
    },
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        ty: Ty,
    },
    Field {
        base: Box<Expr>,
        index: usize,
        ty: Ty,
    },
    TupleField {
        base: Box<Expr>,
        index: usize,
        ty: Ty,
    },
    /// `v[i]`, panics out of bounds.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        ty: Ty,
    },
    /// `base.name(args)`, or `Type::name(args)` with `Assoc` kind.
    Method {
        owner: Box<UserShape>,
        name: String,
        kind: MethodKind,
        base: Option<Box<Expr>>,
        args: Vec<Expr>,
        ty: Ty,
    },
    /// `base.diff_describe()` through the program local trait.
    TraitCall {
        base: Box<Expr>,
    },
    /// `helper(&mut closure, arg)`, a closure handed to a generic helper.
    ApplyCall {
        helper: String,
        closure: String,
        arg: Box<Expr>,
        ty: Ty,
    },
    /// `value?`.
    Try {
        value: Box<Expr>,
        ty: Ty,
    },
    /// `To::from(value)`, or `value.into()` when `bare`.
    Into {
        value: Box<Expr>,
        to: Ty,
        bare: bool,
    },
    Match {
        scrutinee: Box<Expr>,
        /// A slice view, so bindings arrive as references and each arm
        /// clones them.
        by_ref: bool,
        arms: Vec<Arm>,
        ty: Ty,
    },
    /// `{ stmts; tail }`.
    Block {
        stmts: Vec<Stmt>,
        tail: Box<Expr>,
    },
}

impl Expr {
    pub fn ty(&self) -> Ty {
        match self {
            Self::IntLit { width, .. } => Ty::Int(*width),
            Self::BareInt { .. } => Ty::I32,
            Self::FloatLit { width, .. } => Ty::Float(*width),
            Self::BareFloat { .. } => Ty::F64,
            Self::BoolLit { .. } => Ty::Bool,
            Self::CharLit { .. } => Ty::Char,
            Self::StrLit(_) | Self::TraitCall { .. } => Ty::Str,
            Self::VecLit { elem, .. } => Ty::vec_of(elem.clone()),
            Self::OptLit { elem, .. } => Ty::opt_of(elem.clone()),
            Self::MapLit { key, value, .. } => Ty::map_of(key.clone(), value.clone()),
            Self::SetLit { elem, .. } => Ty::set_of(elem.clone()),
            Self::TupleLit(items) => Ty::Tuple(items.iter().map(Expr::ty).collect()),
            Self::ResLit { ok, err, .. } => Ty::res_of(ok.clone(), err.clone()),
            Self::StdErrLit(err) => Ty::StdErr(*err),
            Self::StructLit { shape, .. } | Self::EnumLit { shape, .. } => Ty::User(shape.clone()),
            Self::DefaultOf(ty) | Self::Cast { to: ty, .. } | Self::Into { to: ty, .. } => {
                ty.clone()
            }
            Self::Pipe(pipe) => pipe.ty(),
            Self::Block { tail, .. } => tail.ty(),
            Self::Var { ty, .. }
            | Self::ConstRef { ty, .. }
            | Self::Bin { ty, .. }
            | Self::Unary { ty, .. }
            | Self::Call { ty, .. }
            | Self::If { ty, .. }
            | Self::FnCall { ty, .. }
            | Self::ClosureCall { ty, .. }
            | Self::Field { ty, .. }
            | Self::TupleField { ty, .. }
            | Self::Index { ty, .. }
            | Self::Method { ty, .. }
            | Self::ApplyCall { ty, .. }
            | Self::Try { ty, .. }
            | Self::Match { ty, .. } => ty.clone(),
        }
    }

    /// A binding by name, anything else parenthesized as a temporary.
    fn render_place(&self) -> String {
        match self {
            Self::Var { name, .. } => name.clone(),
            other => format!("({})", other.render()),
        }
    }

    pub fn render(&self) -> String {
        if let Some(text) = self.render_literal() {
            return text;
        }
        match self {
            Self::Pipe(pipe) => pipe.render(),
            Self::FnCall {
                name, args, by_ref, ..
            } => {
                let rendered: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        if by_ref.get(index).copied().unwrap_or(false) {
                            format!("&({})", arg.render())
                        } else {
                            arg.render()
                        }
                    })
                    .collect();
                format!("{name}({})", rendered.join(", "))
            }
            Self::ClosureCall { name, args, .. } => {
                let rendered: Vec<String> = args.iter().map(Expr::render).collect();
                format!("{name}({})", rendered.join(", "))
            }
            // A non copy binding is read through a clone, so the generator
            // never has to track liveness.
            Self::Var { name, ty } if !ty.is_copy() => format!("{name}.clone()"),
            Self::Var { name, .. } => name.clone(),
            Self::ConstRef {
                name, ty, opaque, ..
            } => shield(name, &minimal(ty).render(), *opaque),
            Self::Bin {
                op, left, right, ..
            } => {
                format!("({} {} {})", left.render(), op.token(), right.render())
            }
            Self::Unary { op, value, .. } => format!("({}{})", op.token(), value.render()),
            Self::Cast { value, to } => format!("({} as {})", value.render(), to.rust()),
            Self::Call {
                method,
                recv,
                args,
                fish,
                ..
            } => render_call(method, recv, args, fish.as_ref()),
            // Bare `if a { x } else { y }.len()` parses as
            // `if a { x } else { y.len() }`, so the source would stop matching
            // the tree.
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => format!(
                "(if {} {{ {} }} else {{ {} }})",
                condition.render(),
                then_expr.render(),
                else_expr.render()
            ),
            Self::Field { .. }
            | Self::TupleField { .. }
            | Self::Index { .. }
            | Self::Method { .. }
            | Self::TraitCall { .. }
            | Self::ApplyCall { .. }
            | Self::Try { .. }
            | Self::Into { .. }
            | Self::Match { .. }
            | Self::Block { .. } => self.render_access(),
            _ => unreachable!("every literal renders through render_literal"),
        }
    }

    fn render_access(&self) -> String {
        match self {
            Self::Field { base, index, ty } => {
                let Ty::User(shape) = base.ty() else {
                    return base.render();
                };
                let name = &shape.fields()[*index].name;
                owned(format!("{}.{name}", base.render_place()), ty)
            }
            Self::TupleField { base, index, ty } => {
                owned(format!("{}.{index}", base.render_place()), ty)
            }
            Self::Index { base, index, ty } => {
                owned(format!("{}[{}]", base.render_place(), index.render()), ty)
            }
            Self::Method {
                owner,
                name,
                kind,
                base,
                args,
                ..
            } => {
                let rendered: Vec<String> = args.iter().map(Expr::render).collect();
                match (kind, base) {
                    (MethodKind::Method, Some(base)) => {
                        format!("{}.{name}({})", base.render_place(), rendered.join(", "))
                    }
                    _ => format!("{}::{name}({})", owner.name, rendered.join(", ")),
                }
            }
            Self::TraitCall { base } => format!("{}.diff_describe()", base.render_place()),
            Self::ApplyCall {
                helper,
                closure,
                arg,
                ..
            } => format!("{helper}(&mut {closure}, {})", arg.render()),
            Self::Try { value, .. } => format!("({}?)", value.render()),
            Self::Into {
                value, bare: true, ..
            } => format!("{}.into()", value.render()),
            Self::Into { value, to, .. } => format!("{}::from({})", to.rust(), value.render()),
            Self::Match {
                scrutinee,
                by_ref,
                arms,
                ..
            } => render_match(scrutinee, *by_ref, arms),
            Self::Block { stmts, tail } => {
                let mut out = String::from("{ ");
                for stmt in stmts {
                    out.push_str(stmt.render(&std::collections::BTreeSet::new(), 0).trim());
                    out.push(' ');
                }
                out.push_str(&tail.render());
                out.push_str(" }");
                out
            }
            _ => unreachable!("render_access handles the access nodes only"),
        }
    }

    fn render_literal(&self) -> Option<String> {
        Some(match self {
            Self::IntLit {
                width,
                value,
                opaque,
            } => render_int_lit(*width, *value, *opaque),
            Self::BareInt { value, opaque } => {
                let text = if *value < 0 {
                    format!("({value})")
                } else {
                    value.to_string()
                };
                shield(&text, "0", *opaque)
            }
            Self::FloatLit {
                token,
                opaque: false,
                ..
            } => token.clone(),
            Self::FloatLit {
                width,
                token,
                opaque: true,
            } => format!("diff_opaque_{}({token})", width.rust()),
            Self::BareFloat { token, opaque } => shield(token, "0.0", *opaque),
            Self::BoolLit {
                value,
                opaque: true,
            } => {
                if *value {
                    "diff_opaque_true()".to_string()
                } else {
                    "(!diff_opaque_true())".to_string()
                }
            }
            Self::BoolLit { value, .. } => value.to_string(),
            Self::CharLit {
                value,
                opaque: true,
            } => format!("diff_opaque_char({value:?})"),
            Self::CharLit { value, .. } => format!("{value:?}"),
            Self::StrLit(value) => format!("String::from({value:?})"),
            _ => return self.render_collection_literal(),
        })
    }

    fn render_collection_literal(&self) -> Option<String> {
        Some(match self {
            Self::VecLit { elem, items } if items.is_empty() => {
                format!("Vec::<{}>::new()", elem.rust())
            }
            Self::VecLit { items, .. } => {
                let rendered: Vec<String> = items.iter().map(Expr::render).collect();
                format!("vec![{}]", rendered.join(", "))
            }
            Self::OptLit { elem, value } => match value {
                Some(inner) => format!("Some({})", inner.render()),
                None => format!("None::<{}>", elem.rust()),
            },
            Self::MapLit { key, value, items } if items.is_empty() => {
                format!("HashMap::<{}, {}>::new()", key.rust(), value.rust())
            }
            Self::MapLit { key, value, items } => {
                let inserts: Vec<String> = items
                    .iter()
                    .map(|(entry_key, entry_value)| {
                        format!(
                            "diff_map.insert({}, {});",
                            entry_key.render(),
                            entry_value.render()
                        )
                    })
                    .collect();
                format!(
                    "({{ let mut diff_map: HashMap<{}, {}> = HashMap::new(); {} diff_map }})",
                    key.rust(),
                    value.rust(),
                    inserts.join(" ")
                )
            }
            Self::SetLit { elem, items } if items.is_empty() => {
                format!("HashSet::<{}>::new()", elem.rust())
            }
            Self::SetLit { elem, items } => {
                let inserts: Vec<String> = items
                    .iter()
                    .map(|item| format!("diff_set.insert({});", item.render()))
                    .collect();
                format!(
                    "({{ let mut diff_set: HashSet<{}> = HashSet::new(); {} diff_set }})",
                    elem.rust(),
                    inserts.join(" ")
                )
            }
            Self::TupleLit(items) => {
                let rendered: Vec<String> = items.iter().map(Expr::render).collect();
                match items.len() {
                    1 => format!("({},)", rendered[0]),
                    _ => format!("({})", rendered.join(", ")),
                }
            }
            Self::ResLit { ok, err, value } => match value {
                Ok(inner) => format!("Ok::<{}, {}>({})", ok.rust(), err.rust(), inner.render()),
                Err(inner) => format!("Err::<{}, {}>({})", ok.rust(), err.rust(), inner.render()),
            },
            Self::StdErrLit(err) => match err {
                StdErr::ParseInt => "\"x\".parse::<i32>().unwrap_err()".to_string(),
                StdErr::ParseFloat => "\"x\".parse::<f64>().unwrap_err()".to_string(),
            },
            Self::StructLit {
                shape,
                fields,
                update,
            } => {
                let mut parts: Vec<String> = shape
                    .fields()
                    .iter()
                    .zip(fields)
                    .map(|(field, expr)| format!("{}: {}", field.name, expr.render()))
                    .collect();
                if *update {
                    parts.push("..Default::default()".to_string());
                }
                format!("{} {{ {} }}", shape.name, parts.join(", "))
            }
            Self::EnumLit {
                shape,
                variant,
                payload,
            } => {
                let name = &shape.variants()[*variant].name;
                if payload.is_empty() {
                    format!("{}::{name}", shape.name)
                } else {
                    let rendered: Vec<String> = payload.iter().map(Expr::render).collect();
                    format!("{}::{name}({})", shape.name, rendered.join(", "))
                }
            }
            Self::DefaultOf(ty) => format!("<{}>::default()", ty.rust()),
            _ => return None,
        })
    }
}

fn owned(place: String, ty: &Ty) -> String {
    if ty.is_copy() {
        place
    } else {
        format!("{place}.clone()")
    }
}

/// A branch the overflow lint cannot fold and that states no type, so
/// `rustc` infers the type from the literal alone.
fn shield(text: &str, other: &str, opaque: bool) -> String {
    if opaque {
        format!("(if diff_opaque_true() {{ {text} }} else {{ {other} }})")
    } else {
        text.to_string()
    }
}

fn render_match(scrutinee: &Expr, by_ref: bool, arms: &[Arm]) -> String {
    // The scrutinee is parenthesized because a struct literal is not allowed
    // bare there.
    let view = if by_ref { ".as_slice()" } else { "" };
    let mut out = format!("(match ({}){view} {{ ", scrutinee.render());
    for arm in arms {
        out.push_str(&arm.pat.render());
        if let Some(guard) = &arm.guard {
            out.push_str(&format!(" if {}", guard.render()));
        }
        out.push_str(" => ");
        let mut binds = Vec::new();
        arm.pat.bindings(&mut binds);
        if by_ref && !binds.is_empty() {
            out.push_str("{ ");
            for (name, ty) in &binds {
                // Slice binds are references, the body is typed against owned
                // values.
                let make = if matches!(ty, Ty::Vec(_)) && is_rest(&arm.pat, name) {
                    format!("{name}.to_vec()")
                } else {
                    format!("{name}.clone()")
                };
                out.push_str(&format!("let {name}: {} = {make}; ", ty.rust()));
            }
            out.push_str(&arm.body.render());
            out.push_str(" }");
        } else {
            out.push_str(&arm.body.render());
        }
        out.push_str(", ");
    }
    out.push_str("})");
    out
}

fn is_rest(pat: &Pat, name: &str) -> bool {
    match pat {
        Pat::Slice { rest, .. } => matches!(rest, Some(Some(rest)) if rest == name),
        _ => false,
    }
}

/// Emitted only when used.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Helper {
    I64,
    U64,
    F32,
    F64,
    True,
    Char,
}

impl Helper {
    pub fn definition(self) -> &'static str {
        match self {
            Self::I64 => "fn diff_opaque_i64(value: i64) -> i64 {\n    value\n}\n\n",
            Self::U64 => "fn diff_opaque_u64(value: u64) -> u64 {\n    value\n}\n\n",
            Self::F32 => "fn diff_opaque_f32(value: f32) -> f32 {\n    value\n}\n\n",
            Self::F64 => "fn diff_opaque_f64(value: f64) -> f64 {\n    value\n}\n\n",
            Self::True => "fn diff_opaque_true() -> bool {\n    true\n}\n\n",
            Self::Char => "fn diff_opaque_char(value: char) -> char {\n    value\n}\n\n",
        }
    }
}

fn render_int_lit(width: IntWidth, value: i128, opaque: bool) -> String {
    if !opaque {
        return if value < 0 {
            format!("({value}{})", width.rust())
        } else {
            format!("{value}{}", width.rust())
        };
    }
    if width.is_signed() {
        format!("(diff_opaque_i64({value}) as {})", width.rust())
    } else {
        format!("(diff_opaque_u64({value}) as {})", width.rust())
    }
}

pub fn lookup(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.name == name)
}

fn render_call(method: &str, recv: &Expr, args: &[Expr], fish: Option<&Ty>) -> String {
    let Some(entry) = lookup(method) else {
        return recv.render();
    };
    let rendered_args: Vec<String> = args.iter().map(Expr::render).collect();
    let recv_ty = recv.ty();
    let elem = recv_ty.elem().map(Ty::rust).unwrap_or_default();
    let (key, val) = match recv_ty.key_val().or_else(|| recv_ty.ok_err()) {
        Some((key, value)) => (key.rust(), value.rust()),
        None => (String::new(), String::new()),
    };
    let fish = fish.map(Ty::rust).unwrap_or_default();
    fill(
        entry.template,
        &recv.render(),
        &rendered_args,
        &elem,
        &fish,
        &key,
        &val,
    )
}

/// `{{` and `}}` escape a literal brace like `format!`, so a template can
/// carry a block.
fn fill(
    template: &str,
    recv: &str,
    args: &[String],
    elem: &str,
    fish: &str,
    key_ty: &str,
    val: &str,
) -> String {
    let mut out = String::with_capacity(template.len() + recv.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '}' {
            if chars.peek() == Some(&'}') {
                chars.next();
            }
            out.push('}');
            continue;
        }
        if c != '{' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            out.push('{');
            continue;
        }
        let mut key = String::new();
        for c in chars.by_ref() {
            if c == '}' {
                break;
            }
            key.push(c);
        }
        match key.as_str() {
            "r" => out.push_str(recv),
            "E" => out.push_str(elem),
            "T" => out.push_str(fish),
            "K" => out.push_str(key_ty),
            "V" => out.push_str(val),
            index => match index.parse::<usize>() {
                Ok(index) if index < args.len() => out.push_str(&args[index]),
                _ => {}
            },
        }
    }
    out
}

/// Un-bare every literal in the tree. A receiver like
/// `(if c { 0 } else { 0 })` is as ambiguous as a bare `0`.
pub fn unbare_deep(mut expr: Expr) -> Expr {
    expr = unbare(expr);
    for child in expr.children_mut() {
        let taken = std::mem::replace(
            child,
            Expr::BoolLit {
                value: false,
                opaque: false,
            },
        );
        *child = unbare_deep(taken);
    }
    expr
}

pub fn unbare(expr: Expr) -> Expr {
    match expr {
        Expr::BareInt { value, opaque } => Expr::IntLit {
            width: IntWidth::I32,
            value,
            opaque,
        },
        Expr::BareFloat { token, opaque } => {
            let token = match token.strip_suffix(')') {
                Some(inner) => format!("{inner}f64)"),
                None => format!("{token}f64"),
            };
            Expr::FloatLit {
                width: FloatWidth::F64,
                token,
                opaque,
            }
        }
        other => other,
    }
}

/// The target every shrink step aims at.
pub fn minimal(ty: &Ty) -> Expr {
    match ty {
        Ty::Int(width) => Expr::IntLit {
            width: *width,
            value: 0,
            opaque: false,
        },
        Ty::Float(width) => Expr::FloatLit {
            width: *width,
            token: match width {
                FloatWidth::F32 => "0.0f32".to_string(),
                FloatWidth::F64 => "0.0f64".to_string(),
            },
            opaque: false,
        },
        Ty::Bool => Expr::BoolLit {
            value: false,
            opaque: false,
        },
        Ty::Char => Expr::CharLit {
            value: 'a',
            opaque: false,
        },
        Ty::Str => Expr::StrLit(String::new()),
        Ty::Vec(elem) => Expr::VecLit {
            elem: (**elem).clone(),
            items: Vec::new(),
        },
        Ty::Opt(elem) => Expr::OptLit {
            elem: (**elem).clone(),
            value: None,
        },
        Ty::Map(key, value) => Expr::MapLit {
            key: (**key).clone(),
            value: (**value).clone(),
            items: Vec::new(),
        },
        Ty::Set(elem) => Expr::SetLit {
            elem: (**elem).clone(),
            items: Vec::new(),
        },
        Ty::Tuple(items) => Expr::TupleLit(items.iter().map(minimal).collect()),
        Ty::Res(ok, err) => Expr::ResLit {
            ok: (**ok).clone(),
            err: (**err).clone(),
            value: Ok(Box::new(minimal(ok))),
        },
        Ty::StdErr(err) => Expr::StdErrLit(*err),
        Ty::User(shape) => {
            if shape.is_enum() {
                let variant = &shape.variants()[0];
                Expr::EnumLit {
                    shape: shape.clone(),
                    variant: 0,
                    payload: variant.payload.iter().map(minimal).collect(),
                }
            } else {
                Expr::StructLit {
                    shape: shape.clone(),
                    fields: shape.fields().iter().map(|f| minimal(&f.ty)).collect(),
                    update: false,
                }
            }
        }
    }
}
