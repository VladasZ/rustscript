//! Renders an `Expr` tree to Rust source text.

use crate::lang::pat::Pat;
use crate::lang::ty::{IntWidth, StdErr, Ty};
use crate::lang::user::MethodKind;

use super::expr::{Arm, Expr, MemKind, ReadMode, VecTakeKind, lookup, minimal};

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
            Self::VecLit { elem, .. } | Self::VecRepeat { elem, .. } => Ty::vec_of(elem.clone()),
            Self::OptLit { elem, .. } => Ty::opt_of(elem.clone()),
            Self::MapLit { key, value, .. } => Ty::map_of(key.clone(), value.clone()),
            Self::SetLit { elem, .. } => Ty::set_of(elem.clone()),
            Self::TupleLit(items) => Ty::Tuple(items.iter().map(Expr::ty).collect()),
            Self::ResLit { ok, err, .. } => Ty::res_of(ok.clone(), err.clone()),
            Self::StdErrLit(err) => Ty::StdErr(*err),
            Self::TraceLit(_) => Ty::Trace,
            Self::VecTake { elem, kind, .. } => match kind {
                VecTakeKind::Pop => Ty::opt_of(elem.clone()),
                VecTakeKind::Remove(_) | VecTakeKind::SwapRemove(_) => elem.clone(),
            },
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
            | Self::Mem { ty, .. }
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
            Self::Var { name, ty, mode } => owned(name.clone(), ty, *mode),
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
            // Bare `if a { x } else { y }.len()` parses as `if a { x } else { y.len() }`, so the
            // source would stop matching the tree.
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
            | Self::Block { .. }
            | Self::Mem { .. }
            | Self::VecTake { .. } => self.render_access(),
            _ => unreachable!("every literal renders through render_literal"),
        }
    }

    fn render_access(&self) -> String {
        match self {
            Self::Field {
                base,
                index,
                ty,
                mode,
            } => {
                let Ty::User(shape) = base.ty() else {
                    return base.render();
                };
                let name = &shape.fields()[*index].name;
                owned(format!("{}.{name}", base.render_place()), ty, *mode)
            }
            Self::TupleField {
                base,
                index,
                ty,
                mode,
            } => owned(format!("{}.{index}", base.render_place()), ty, *mode),
            // an index can't be moved out of, so it always clones
            Self::Index { base, index, ty } => owned(
                format!("{}[{}]", base.render_place(), index.render()),
                ty,
                ReadMode::Clone,
            ),
            Self::Mem { name, kind, .. } => match kind {
                MemKind::Take => format!("std::mem::take(&mut {name})"),
                MemKind::Replace(value) => {
                    format!("std::mem::replace(&mut {name}, {})", value.render())
                }
                MemKind::OptTake => format!("{name}.take()"),
            },
            Self::VecTake { name, kind, .. } => match kind {
                VecTakeKind::Pop => format!("{name}.pop()"),
                VecTakeKind::Remove(index) => format!("{name}.remove({index}usize)"),
                VecTakeKind::SwapRemove(index) => format!("{name}.swap_remove({index}usize)"),
            },
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
            Self::TraceLit(id) => format!("DiffTrace({id})"),
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
            Self::VecRepeat { item, count, .. } => {
                format!("vec![{}; {count}usize]", item.render())
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

fn owned(place: String, ty: &Ty, mode: ReadMode) -> String {
    if ty.is_copy() || mode == ReadMode::Move {
        place
    } else {
        format!("{place}.clone()")
    }
}

/// A branch the overflow lint can't fold and that states no type, so `rustc` infers the type from
/// the literal alone.
fn shield(text: &str, other: &str, opaque: bool) -> String {
    if opaque {
        format!("(if diff_opaque_true() {{ {text} }} else {{ {other} }})")
    } else {
        text.to_string()
    }
}

fn render_match(scrutinee: &Expr, by_ref: bool, arms: &[Arm]) -> String {
    // the scrutinee is parenthesized because a struct literal is not allowed bare there
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
                // slice binds are references, the body is typed against owned values
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

/// `{{` and `}}` escape a literal brace like `format!`, so a template can carry a block.
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
