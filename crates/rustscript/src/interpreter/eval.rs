use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use syn::punctuated::Punctuated;
use syn::{BinOp, Block, Expr, FnArg, Lit, Pat, Stmt, UnOp};

use std::collections::BTreeMap;

use super::value::{ClosureData, MapKey, Value};
use super::{Flow, Frame, Interp, flow};

impl Interp {
    /// Bind arguments and run a function or method body.
    pub(super) fn call_fn_body(
        &self,
        block: &Block,
        sig: &syn::Signature,
        args: &[Value],
        frame: &mut Frame,
    ) -> Result<Value> {
        let params: Vec<&Pat> = sig
            .inputs
            .iter()
            .filter_map(|a| match a {
                FnArg::Typed(t) => Some(&*t.pat),
                FnArg::Receiver(_) => None,
            })
            .collect();
        if params.len() != args.len() {
            bail!(
                "function `{}` expects {} args but got {}",
                sig.ident,
                params.len(),
                args.len()
            );
        }
        for (pat, val) in params.iter().zip(args.iter()) {
            self.bind_pattern(pat, val.clone(), frame)?;
        }
        match self.eval_block(block, frame)? {
            Flow::Value(v) | Flow::Return(v) => Ok(v),
            Flow::Break(_) | Flow::Continue => bail!("break or continue outside a loop"),
        }
    }

    /// Turn a dynamic value into `ty` when `ty` names a known struct, walking
    /// `Vec<T>`, `Option<T>`, and smart pointers. Anything else is unchanged.
    pub(super) fn coerce_value(&self, value: Value, ty: &syn::Type) -> Value {
        let syn::Type::Path(p) = ty else {
            return value;
        };
        let Some(seg) = p.path.segments.last() else {
            return value;
        };
        let name = seg.ident.to_string();
        match name.as_str() {
            "Vec" | "VecDeque" => {
                if let (Value::Vec(items), Some(inner)) = (&value, first_generic_type(seg)) {
                    let out = items
                        .borrow()
                        .iter()
                        .map(|v| self.coerce_value(v.clone(), inner))
                        .collect();
                    return Value::vec(out);
                }
                value
            }
            "Option" => {
                if let (Value::Enum { enum_name, variant, data }, Some(inner)) =
                    (&value, first_generic_type(seg))
                    && enum_name == "Option"
                    && variant == "Some"
                {
                    let coerced = self.coerce_value(
                        data.borrow().first().cloned().unwrap_or(Value::Unit),
                        inner,
                    );
                    return Value::some(coerced);
                }
                value
            }
            "Box" | "Rc" | "Arc" => match first_generic_type(seg) {
                Some(inner) => self.coerce_value(value, inner),
                None => value,
            },
            _ => {
                if let Some(def) = self.structs.get(&name).cloned()
                    && let Value::Map(map) = &value
                {
                    return self.struct_from_map(&name, &def, &map.borrow());
                }
                value
            }
        }
    }

    /// If `value` is `Ok(x)` coerce `x`, otherwise coerce `value` directly.
    /// Used for `from_str::<T>()` style turbofish calls.
    pub(super) fn coerce_result(&self, value: Value, ty: &syn::Type) -> Value {
        if let Value::Enum { enum_name, variant, data } = &value
            && enum_name == "Result"
            && variant == "Ok"
        {
            let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
            return Value::ok(self.coerce_value(inner, ty));
        }
        self.coerce_value(value, ty)
    }

    fn struct_from_map(
        &self,
        name: &str,
        def: &syn::ItemStruct,
        map: &BTreeMap<MapKey, Value>,
    ) -> Value {
        let mut fields = BTreeMap::new();
        if let syn::Fields::Named(named) = &def.fields {
            for f in &named.named {
                let Some(ident) = &f.ident else { continue };
                let fname = ident.to_string();
                let raw = map
                    .get(&MapKey::Str(fname.clone()))
                    .cloned()
                    .unwrap_or_else(Value::none);
                fields.insert(fname, self.coerce_value(raw, &f.ty));
            }
        }
        Value::Struct {
            name: name.to_string(),
            fields: std::rc::Rc::new(std::cell::RefCell::new(fields)),
        }
    }

    /// Call a closure with already-evaluated arguments.
    pub(super) fn call_closure(&self, clo: &ClosureData, args: &[Value]) -> Result<Value> {
        if clo.params.len() != args.len() {
            bail!(
                "closure expects {} args but got {}",
                clo.params.len(),
                args.len()
            );
        }
        let mut frame = Frame::new();
        for (k, v) in &clo.captured {
            frame.define(k, v.clone());
        }
        frame.push();
        for (p, a) in clo.params.iter().zip(args.iter()) {
            self.bind_pattern(p, a.clone(), &mut frame)?;
        }
        match self.eval_expr(&clo.body, &mut frame)? {
            Flow::Value(v) | Flow::Return(v) => Ok(v),
            _ => bail!("break or continue is not allowed inside a closure"),
        }
    }

    pub(super) fn eval_block(&self, block: &Block, frame: &mut Frame) -> Result<Flow> {
        frame.push();
        let result = self.eval_block_inner(block, frame);
        frame.pop();
        result
    }

    fn eval_block_inner(&self, block: &Block, frame: &mut Frame) -> Result<Flow> {
        let mut last = Value::Unit;
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i + 1 == block.stmts.len();
            match self.eval_stmt(stmt, frame)? {
                Flow::Value(v) => last = if is_last { v } else { Value::Unit },
                other => return Ok(other),
            }
        }
        Ok(Flow::Value(last))
    }

    fn eval_stmt(&self, stmt: &Stmt, frame: &mut Frame) -> Result<Flow> {
        match stmt {
            Stmt::Local(local) => {
                let value = match &local.init {
                    Some(init) => flow!(self.eval_expr(&init.expr, frame)),
                    None => Value::Unit,
                };
                // A typed binding like `let c: Config = ...` turns a dynamic
                // json value into the annotated struct.
                let value = if let Pat::Type(t) = &local.pat {
                    self.coerce_value(value, &t.ty)
                } else {
                    value
                };
                self.bind_pattern(&local.pat, value, frame)?;
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Expr(expr, semi) => {
                let v = flow!(self.eval_expr(expr, frame));
                Ok(Flow::Value(if semi.is_some() { Value::Unit } else { v }))
            }
            Stmt::Item(item) => {
                // Local items were not collected at load time; only a few make sense.
                if let syn::Item::Fn(_) = item {
                    bail!("unsupported feature: nested functions");
                }
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Macro(m) => self.eval_macro(&m.mac, frame),
        }
    }

    pub(super) fn eval_expr(&self, expr: &Expr, frame: &mut Frame) -> Result<Flow> {
        let value = match expr {
            Expr::Lit(lit) => self.eval_lit(&lit.lit)?,
            Expr::Paren(p) => flow!(self.eval_expr(&p.expr, frame)),
            Expr::Group(g) => flow!(self.eval_expr(&g.expr, frame)),
            Expr::Block(b) => return self.eval_block(&b.block, frame),
            Expr::Path(p) => self.eval_path(&p.path, frame)?,
            Expr::Reference(r) => flow!(self.eval_expr(&r.expr, frame)),
            Expr::Unary(u) => {
                let v = flow!(self.eval_expr(&u.expr, frame));
                self.eval_unary(&u.op, v)?
            }
            Expr::Binary(b) => return self.eval_binary(b, frame),
            Expr::Assign(a) => {
                let val = flow!(self.eval_expr(&a.right, frame));
                self.assign(&a.left, val, frame)?;
                Value::Unit
            }
            Expr::If(if_expr) => return self.eval_if(if_expr, frame),
            Expr::While(w) => return self.eval_while(w, frame),
            Expr::ForLoop(f) => return self.eval_for(f, frame),
            Expr::Loop(l) => return self.eval_loop(l, frame),
            Expr::Match(m) => return self.eval_match(m, frame),
            Expr::Return(r) => {
                let v = match &r.expr {
                    Some(e) => flow!(self.eval_expr(e, frame)),
                    None => Value::Unit,
                };
                return Ok(Flow::Return(v));
            }
            Expr::Break(b) => {
                let v = match &b.expr {
                    Some(e) => flow!(self.eval_expr(e, frame)),
                    None => Value::Unit,
                };
                return Ok(Flow::Break(v));
            }
            Expr::Continue(_) => return Ok(Flow::Continue),
            Expr::Call(c) => return self.eval_call(c, frame),
            Expr::MethodCall(m) => {
                let recv = flow!(self.eval_expr(&m.receiver, frame));
                let mut args = Vec::new();
                for a in &m.args {
                    args.push(flow!(self.eval_expr(a, frame)));
                }
                self.eval_method(recv, &m.method.to_string(), args, frame)?
            }
            Expr::Macro(m) => return self.eval_macro(&m.mac, frame),
            Expr::Tuple(t) => {
                let mut items = Vec::new();
                for e in &t.elems {
                    items.push(flow!(self.eval_expr(e, frame)));
                }
                Value::Tuple(Rc::new(RefCell::new(items)))
            }
            Expr::Array(a) => {
                let mut items = Vec::new();
                for e in &a.elems {
                    items.push(flow!(self.eval_expr(e, frame)));
                }
                Value::vec(items)
            }
            Expr::Repeat(r) => {
                let v = flow!(self.eval_expr(&r.expr, frame));
                let n = match flow!(self.eval_expr(&r.len, frame)) {
                    Value::Int(n) => n as usize,
                    _ => bail!("array repeat length must be an integer"),
                };
                Value::vec(std::iter::repeat_n(v, n).collect())
            }
            Expr::Index(idx) => {
                let base = flow!(self.eval_expr(&idx.expr, frame));
                let key = flow!(self.eval_expr(&idx.index, frame));
                self.index(&base, &key)?
            }
            Expr::Field(f) => {
                let base = flow!(self.eval_expr(&f.base, frame));
                self.field(&base, &f.member)?
            }
            Expr::Struct(s) => self.eval_struct_literal(s, frame)?,
            Expr::Range(r) => self.eval_range(r, frame)?,
            Expr::Try(t) => {
                let v = flow!(self.eval_expr(&t.expr, frame));
                match self.eval_try(v)? {
                    Ok(v) => v,
                    Err(early) => return Ok(Flow::Return(early)),
                }
            }
            Expr::Cast(c) => {
                let v = flow!(self.eval_expr(&c.expr, frame));
                self.eval_cast(v, &c.ty)?
            }
            Expr::Closure(c) => {
                let params = c.inputs.iter().cloned().collect();
                Value::Closure(Rc::new(ClosureData {
                    params,
                    body: (*c.body).clone(),
                    captured: frame.snapshot(),
                }))
            }
            Expr::Async(_) | Expr::Await(_) => {
                bail!("unsupported feature: async is not supported")
            }
            Expr::Unsafe(_) => bail!("unsupported feature: unsafe is not supported"),
            other => bail!("unsupported expression: {}", expr_kind(other)),
        };
        Ok(Flow::Value(value))
    }

    fn eval_lit(&self, lit: &Lit) -> Result<Value> {
        Ok(match lit {
            Lit::Int(i) => Value::Int(i.base10_parse::<i128>()?),
            Lit::Float(f) => Value::Float(f.base10_parse::<f64>()?),
            Lit::Bool(b) => Value::Bool(b.value),
            Lit::Str(s) => Value::str(s.value()),
            Lit::Char(c) => Value::Char(c.value()),
            Lit::Byte(b) => Value::Int(b.value() as i128),
            other => bail!("unsupported literal: {:?}", other),
        })
    }

    fn eval_unary(&self, op: &UnOp, v: Value) -> Result<Value> {
        Ok(match (op, v) {
            (UnOp::Neg(_), Value::Int(i)) => Value::Int(-i),
            (UnOp::Neg(_), Value::Float(f)) => Value::Float(-f),
            (UnOp::Not(_), Value::Bool(b)) => Value::Bool(!b),
            (UnOp::Not(_), Value::Int(i)) => Value::Int(!i),
            (UnOp::Deref(_), v) => v,
            (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
        })
    }

    fn eval_binary(&self, b: &syn::ExprBinary, frame: &mut Frame) -> Result<Flow> {
        // Short circuiting logical operators.
        match b.op {
            BinOp::And(_) => {
                let l = flow!(self.eval_expr(&b.left, frame));
                if !l.is_truthy() {
                    return Ok(Flow::Value(Value::Bool(false)));
                }
                return Ok(Flow::Value(flow!(self.eval_expr(&b.right, frame))));
            }
            BinOp::Or(_) => {
                let l = flow!(self.eval_expr(&b.left, frame));
                if l.is_truthy() {
                    return Ok(Flow::Value(Value::Bool(true)));
                }
                return Ok(Flow::Value(flow!(self.eval_expr(&b.right, frame))));
            }
            _ => {}
        }

        // Compound assignment: evaluate, combine, store back.
        if let Some(inner) = assign_op(&b.op) {
            let cur = flow!(self.eval_expr(&b.left, frame));
            let rhs = flow!(self.eval_expr(&b.right, frame));
            let combined = arith(inner, cur, rhs)?;
            self.assign(&b.left, combined, frame)?;
            return Ok(Flow::Value(Value::Unit));
        }

        let l = flow!(self.eval_expr(&b.left, frame));
        let r = flow!(self.eval_expr(&b.right, frame));
        Ok(Flow::Value(binop(&b.op, l, r)?))
    }

    fn eval_if(&self, if_expr: &syn::ExprIf, frame: &mut Frame) -> Result<Flow> {
        // `if let` support.
        if let Expr::Let(let_expr) = &*if_expr.cond {
            let scrutinee = flow!(self.eval_expr(&let_expr.expr, frame));
            frame.push();
            let matched = self.try_bind(&let_expr.pat, &scrutinee, frame);
            let out = if matched {
                self.eval_block_inner(&if_expr.then_branch, frame)
            } else {
                Ok(Flow::Value(Value::Unit))
            };
            frame.pop();
            let out = out?;
            if matched {
                return Ok(out);
            }
        } else {
            let cond = flow!(self.eval_expr(&if_expr.cond, frame));
            if cond.is_truthy() {
                return self.eval_block(&if_expr.then_branch, frame);
            }
        }
        match &if_expr.else_branch {
            Some((_, else_expr)) => self.eval_expr(else_expr, frame),
            None => Ok(Flow::Value(Value::Unit)),
        }
    }

    fn eval_while(&self, w: &syn::ExprWhile, frame: &mut Frame) -> Result<Flow> {
        loop {
            let cond = flow!(self.eval_expr(&w.cond, frame));
            if !cond.is_truthy() {
                break;
            }
            match self.eval_block(&w.body, frame)? {
                Flow::Break(_) => break,
                Flow::Return(v) => return Ok(Flow::Return(v)),
                _ => {}
            }
        }
        Ok(Flow::Value(Value::Unit))
    }

    fn eval_loop(&self, l: &syn::ExprLoop, frame: &mut Frame) -> Result<Flow> {
        loop {
            match self.eval_block(&l.body, frame)? {
                Flow::Break(v) => return Ok(Flow::Value(v)),
                Flow::Return(v) => return Ok(Flow::Return(v)),
                _ => {}
            }
        }
    }

    fn eval_for(&self, f: &syn::ExprForLoop, frame: &mut Frame) -> Result<Flow> {
        let iterable = flow!(self.eval_expr(&f.expr, frame));
        let items = self.into_iter_items(iterable)?;
        for item in items {
            frame.push();
            self.bind_pattern(&f.pat, item, frame)?;
            let flow = self.eval_block_inner(&f.body, frame);
            frame.pop();
            match flow? {
                Flow::Break(_) => break,
                Flow::Return(v) => return Ok(Flow::Return(v)),
                _ => {}
            }
        }
        Ok(Flow::Value(Value::Unit))
    }

    /// Expand any iterable into a concrete list of items.
    pub(super) fn into_iter_items(&self, v: Value) -> Result<Vec<Value>> {
        Ok(match v {
            Value::Vec(items) => items.borrow().clone(),
            Value::Tuple(items) => items.borrow().clone(),
            Value::Range {
                start,
                end,
                inclusive,
            } => {
                let end = if inclusive { end + 1 } else { end };
                (start..end).map(Value::Int).collect()
            }
            Value::Map(map) => map
                .borrow()
                .iter()
                .map(|(k, v)| {
                    Value::Tuple(Rc::new(RefCell::new(vec![k.to_value(), v.clone()])))
                })
                .collect(),
            Value::Str(s) => s.borrow().chars().map(Value::Char).collect(),
            other => bail!("{} is not iterable", other.type_name()),
        })
    }

    fn eval_match(&self, m: &syn::ExprMatch, frame: &mut Frame) -> Result<Flow> {
        let scrutinee = flow!(self.eval_expr(&m.expr, frame));
        for arm in &m.arms {
            frame.push();
            let matched = self.try_bind(&arm.pat, &scrutinee, frame);
            if matched {
                if let Some((_, guard)) = &arm.guard {
                    let g = self.eval_expr(guard, frame);
                    let pass = match g {
                        Ok(Flow::Value(v)) => v.is_truthy(),
                        other => {
                            frame.pop();
                            return other;
                        }
                    };
                    if !pass {
                        frame.pop();
                        continue;
                    }
                }
                let out = self.eval_expr(&arm.body, frame);
                frame.pop();
                return out;
            }
            frame.pop();
        }
        bail!("no match arm matched value {}", scrutinee.debug())
    }

    fn eval_range(&self, r: &syn::ExprRange, frame: &mut Frame) -> Result<Value> {
        let start = match &r.start {
            Some(e) => match flow_value(self.eval_expr(e, frame)?)? {
                Value::Int(i) => i,
                _ => bail!("range bound must be an integer"),
            },
            None => 0,
        };
        let end = match &r.end {
            Some(e) => match flow_value(self.eval_expr(e, frame)?)? {
                Value::Int(i) => i,
                _ => bail!("range bound must be an integer"),
            },
            None => bail!("open ended ranges are not supported"),
        };
        let inclusive = matches!(r.limits, syn::RangeLimits::Closed(_));
        Ok(Value::Range {
            start,
            end,
            inclusive,
        })
    }

    fn eval_try(&self, v: Value) -> Result<std::result::Result<Value, Value>> {
        match v {
            Value::Enum {
                enum_name,
                variant,
                data,
            } if enum_name == "Result" => {
                let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
                if variant == "Ok" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::err(inner)))
                }
            }
            Value::Enum {
                enum_name,
                variant,
                data,
            } if enum_name == "Option" => {
                let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
                if variant == "Some" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::none()))
                }
            }
            other => bail!("the `?` operator needs a Result or Option, got {}", other.type_name()),
        }
    }

    fn eval_cast(&self, v: Value, ty: &syn::Type) -> Result<Value> {
        let target = match ty {
            syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        let target = target.unwrap_or_default();
        Ok(match target.as_str() {
            "f64" | "f32" => Value::Float(match v {
                Value::Int(i) => i as f64,
                Value::Float(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }),
            "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32"
            | "i64" | "i128" => Value::Int(match v {
                Value::Int(i) => i,
                Value::Float(f) => f as i128,
                Value::Char(c) => c as i128,
                Value::Bool(b) => b as i128,
                other => bail!("cannot cast {} to integer", other.type_name()),
            }),
            "char" => match v {
                Value::Int(i) => Value::Char(
                    char::from_u32(i as u32).ok_or_else(|| anyhow!("invalid char code {i}"))?,
                ),
                Value::Char(c) => Value::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            },
            other => bail!("unsupported cast target: {other}"),
        })
    }

    // -- lvalues -----------------------------------------------------------

    fn assign(&self, target: &Expr, val: Value, frame: &mut Frame) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                if !frame.set(&name, val) {
                    bail!("assignment to unknown variable `{name}`");
                }
            }
            Expr::Index(idx) => {
                let base = flow_value(self.eval_expr(&idx.expr, frame)?)?;
                let key = flow_value(self.eval_expr(&idx.index, frame)?)?;
                match base {
                    Value::Vec(items) => {
                        let i = as_index(&key)?;
                        let mut items = items.borrow_mut();
                        if i >= items.len() {
                            bail!("index {i} out of bounds (len {})", items.len());
                        }
                        items[i] = val;
                    }
                    Value::Map(map) => {
                        let k = key
                            .as_key()
                            .ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                        map.borrow_mut().insert(k, val);
                    }
                    other => bail!("cannot index-assign into {}", other.type_name()),
                }
            }
            Expr::Field(f) => {
                let base = flow_value(self.eval_expr(&f.base, frame)?)?;
                match (base, &f.member) {
                    (Value::Struct { fields, .. }, syn::Member::Named(name)) => {
                        fields.borrow_mut().insert(name.to_string(), val);
                    }
                    (Value::Tuple(items), syn::Member::Unnamed(i)) => {
                        items.borrow_mut()[i.index as usize] = val;
                    }
                    (b, _) => bail!("cannot assign to field of {}", b.type_name()),
                }
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                self.assign(&u.expr, val, frame)?;
            }
            _ => bail!("invalid assignment target"),
        }
        Ok(())
    }

    fn index(&self, base: &Value, key: &Value) -> Result<Value> {
        Ok(match base {
            Value::Vec(items) => {
                let i = as_index(key)?;
                items
                    .borrow()
                    .get(i)
                    .cloned()
                    .ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Str(s) => {
                let i = as_index(key)?;
                s.borrow()
                    .chars()
                    .nth(i)
                    .map(Value::Char)
                    .ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Tuple(items) => {
                let i = as_index(key)?;
                items
                    .borrow()
                    .get(i)
                    .cloned()
                    .ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Map(map) => {
                let k = key
                    .as_key()
                    .ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow()
                    .get(&k)
                    .cloned()
                    .ok_or_else(|| anyhow!("key not found"))?
            }
            Value::Struct { name, fields } if name == "Captures" => {
                super::builtins::capture_index(fields, key)?
            }
            other => bail!("cannot index {}", other.type_name()),
        })
    }

    fn field(&self, base: &Value, member: &syn::Member) -> Result<Value> {
        match (base, member) {
            (Value::Struct { fields, .. }, syn::Member::Named(name)) => fields
                .borrow()
                .get(&name.to_string())
                .cloned()
                .ok_or_else(|| anyhow!("no field `{name}`")),
            (Value::Tuple(items), syn::Member::Unnamed(i)) => items
                .borrow()
                .get(i.index as usize)
                .cloned()
                .ok_or_else(|| anyhow!("no field `{}`", i.index)),
            (Value::Struct { fields, .. }, syn::Member::Unnamed(i)) => fields
                .borrow()
                .get(&i.index.to_string())
                .cloned()
                .ok_or_else(|| anyhow!("no field `{}`", i.index)),
            (b, _) => bail!("cannot access field of {}", b.type_name()),
        }
    }

    // -- patterns ----------------------------------------------------------

    pub(super) fn bind_pattern(&self, pat: &Pat, val: Value, frame: &mut Frame) -> Result<()> {
        if !self.try_bind(pat, &val, frame) {
            bail!("pattern did not match value {}", val.debug());
        }
        Ok(())
    }

    /// Try to match `pat` against `val`, binding names into `frame`. Returns
    /// false if the pattern does not match, in which case partial bindings may
    /// have been added and the caller should discard the scope.
    fn try_bind(&self, pat: &Pat, val: &Value, frame: &mut Frame) -> bool {
        match pat {
            Pat::Wild(_) => true,
            Pat::Rest(_) => true,
            Pat::Ident(id) => {
                if let Some(sub) = &id.subpat {
                    if !self.try_bind(&sub.1, val, frame) {
                        return false;
                    }
                }
                frame.define(&id.ident.to_string(), val.clone());
                true
            }
            Pat::Lit(lit) => match self.eval_lit_pattern(lit) {
                Some(expected) => expected.eq_value(val),
                None => false,
            },
            Pat::Paren(p) => self.try_bind(&p.pat, val, frame),
            Pat::Reference(r) => self.try_bind(&r.pat, val, frame),
            Pat::Type(t) => self.try_bind(&t.pat, val, frame),
            Pat::Tuple(t) => {
                let items = match val {
                    Value::Tuple(items) => items.borrow(),
                    _ => return false,
                };
                self.bind_seq(t.elems.iter(), &items, frame)
            }
            Pat::TupleStruct(ts) => {
                let name = ts.path.segments.last().map(|s| s.ident.to_string());
                match val {
                    Value::Enum { variant, data, .. } => {
                        if name.as_deref() != Some(variant.as_str()) {
                            return false;
                        }
                        let data = data.borrow();
                        self.bind_seq(ts.elems.iter(), &data, frame)
                    }
                    Value::Struct { fields, .. } => {
                        let vals: Vec<Value> = fields.borrow().values().cloned().collect();
                        self.bind_seq(ts.elems.iter(), &vals, frame)
                    }
                    _ => false,
                }
            }
            Pat::Path(p) => {
                let name = p.path.segments.last().map(|s| s.ident.to_string());
                match val {
                    Value::Enum { variant, .. } => name.as_deref() == Some(variant.as_str()),
                    _ => false,
                }
            }
            Pat::Struct(s) => {
                let name = s.path.segments.last().map(|s| s.ident.to_string());
                let fields = match val {
                    Value::Struct {
                        name: n, fields, ..
                    } => {
                        if let Some(pn) = &name
                            && pn != n
                        {
                            return false;
                        }
                        fields.borrow()
                    }
                    _ => return false,
                };
                for f in &s.fields {
                    let key = match &f.member {
                        syn::Member::Named(n) => n.to_string(),
                        syn::Member::Unnamed(i) => i.index.to_string(),
                    };
                    match fields.get(&key) {
                        Some(v) => {
                            if !self.try_bind(&f.pat, v, frame) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            }
            Pat::Or(or) => or.cases.iter().any(|c| self.try_bind(c, val, frame)),
            Pat::Slice(s) => {
                let items = match val {
                    Value::Vec(items) => items.borrow(),
                    _ => return false,
                };
                self.bind_seq(s.elems.iter(), &items, frame)
            }
            _ => false,
        }
    }

    fn bind_seq<'a>(
        &self,
        pats: impl Iterator<Item = &'a Pat>,
        vals: &[Value],
        frame: &mut Frame,
    ) -> bool {
        let pats: Vec<&Pat> = pats.collect();
        if pats.iter().any(|p| matches!(p, Pat::Rest(_))) {
            // Only a trailing or leading rest is handled.
            let head = pats.iter().take_while(|p| !matches!(p, Pat::Rest(_)));
            let head_len = head.clone().count();
            for (p, v) in head.zip(vals.iter()) {
                if !self.try_bind(p, v, frame) {
                    return false;
                }
            }
            let tail: Vec<&&Pat> = pats.iter().skip(head_len + 1).collect();
            for (p, v) in tail.iter().zip(vals.iter().rev()) {
                if !self.try_bind(p, v, frame) {
                    return false;
                }
            }
            return true;
        }
        if pats.len() != vals.len() {
            return false;
        }
        pats.iter()
            .zip(vals.iter())
            .all(|(p, v)| self.try_bind(p, v, frame))
    }

    fn eval_lit_pattern(&self, lit: &syn::PatLit) -> Option<Value> {
        self.eval_lit(&lit.lit).ok()
    }

    fn eval_struct_literal(&self, s: &syn::ExprStruct, frame: &mut Frame) -> Result<Value> {
        let name = s
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_default();

        // Enum struct variant, or Option/Result written in path form.
        if s.path.segments.len() >= 2 || self.enums.contains_key(&name) {
            // Fall through to struct handling below for now.
        }

        let mut fields = std::collections::BTreeMap::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            let v = flow_value(self.eval_expr(&f.expr, frame)?)?;
            fields.insert(key, v);
        }
        if let Some(rest) = &s.rest {
            let base = flow_value(self.eval_expr(rest, frame)?)?;
            if let Value::Struct { fields: bf, .. } = base {
                for (k, v) in bf.borrow().iter() {
                    fields.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        Ok(Value::Struct {
            name,
            fields: Rc::new(RefCell::new(fields)),
        })
    }

    // -- macros ------------------------------------------------------------

    fn eval_macro(&self, mac: &syn::Macro, frame: &mut Frame) -> Result<Flow> {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let value = match name.as_str() {
            "println" | "print" | "eprintln" | "eprint" => {
                let text = self.expand_format(mac, frame)?;
                match name.as_str() {
                    "println" => println!("{text}"),
                    "print" => print!("{text}"),
                    "eprintln" => eprintln!("{text}"),
                    _ => eprint!("{text}"),
                }
                Value::Unit
            }
            "format" => Value::str(self.expand_format(mac, frame)?),
            "vec" => {
                if let Ok(rep) = mac.parse_body_with(parse_vec_repeat) {
                    let v = flow_value(self.eval_expr(&rep.0, frame)?)?;
                    let n = match flow_value(self.eval_expr(&rep.1, frame)?)? {
                        Value::Int(n) => n as usize,
                        _ => bail!("vec! repeat count must be an integer"),
                    };
                    Value::vec(std::iter::repeat_n(v, n).collect())
                } else {
                    let exprs = mac.parse_body_with(
                        Punctuated::<Expr, syn::Token![,]>::parse_terminated,
                    )?;
                    let mut items = Vec::new();
                    for e in &exprs {
                        items.push(flow_value(self.eval_expr(e, frame)?)?);
                    }
                    Value::vec(items)
                }
            }
            "panic" => {
                let text = self.expand_format(mac, frame)?;
                bail!("panicked: {text}");
            }
            "assert" => {
                let args =
                    mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
                let cond = args
                    .first()
                    .ok_or_else(|| anyhow!("assert! needs a condition"))?;
                if !flow_value(self.eval_expr(cond, frame)?)?.is_truthy() {
                    bail!("assertion failed");
                }
                Value::Unit
            }
            "assert_eq" | "assert_ne" => {
                let args =
                    mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
                let mut it = args.iter();
                let a = flow_value(self.eval_expr(
                    it.next().ok_or_else(|| anyhow!("assert needs two args"))?,
                    frame,
                )?)?;
                let b = flow_value(self.eval_expr(
                    it.next().ok_or_else(|| anyhow!("assert needs two args"))?,
                    frame,
                )?)?;
                let eq = a.eq_value(&b);
                if eq != (name == "assert_eq") {
                    bail!("assertion failed: {} vs {}", a.debug(), b.debug());
                }
                Value::Unit
            }
            "anyhow" => Value::err(Value::str(self.expand_format(mac, frame)?)),
            "bail" => {
                let err = Value::err(Value::str(self.expand_format(mac, frame)?));
                return Ok(Flow::Return(err));
            }
            "ensure" => {
                let args =
                    mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
                let cond = args
                    .first()
                    .ok_or_else(|| anyhow!("ensure! needs a condition"))?;
                if !flow_value(self.eval_expr(cond, frame)?)?.is_truthy() {
                    let msg = args
                        .iter()
                        .nth(1)
                        .map(|e| flow_value(self.eval_expr(e, frame)?).map(|v| v.display()))
                        .transpose()?
                        .unwrap_or_else(|| "condition failed".into());
                    return Ok(Flow::Return(Value::err(Value::str(msg))));
                }
                Value::Unit
            }
            "dbg" => {
                let args =
                    mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
                let mut last = Value::Unit;
                for e in &args {
                    last = flow_value(self.eval_expr(e, frame)?)?;
                    eprintln!("[dbg] {}", last.debug());
                }
                last
            }
            other => bail!("unsupported macro: {other}!"),
        };
        Ok(Flow::Value(value))
    }
}

/// Helper to turn an evaluated Flow into a plain value, erroring on stray control flow.
pub(super) fn flow_value(flow: Flow) -> Result<Value> {
    match flow {
        Flow::Value(v) => Ok(v),
        _ => bail!("unexpected break, continue, or return"),
    }
}

fn as_index(key: &Value) -> Result<usize> {
    match key {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        Value::Int(i) => bail!("negative index {i}"),
        other => bail!("index must be an integer, got {}", other.type_name()),
    }
}

fn assign_op(op: &BinOp) -> Option<BinOp> {
    use syn::token;
    let sp = proc_macro2::Span::call_site();
    Some(match op {
        BinOp::AddAssign(_) => BinOp::Add(token::Plus { spans: [sp] }),
        BinOp::SubAssign(_) => BinOp::Sub(token::Minus { spans: [sp] }),
        BinOp::MulAssign(_) => BinOp::Mul(token::Star { spans: [sp] }),
        BinOp::DivAssign(_) => BinOp::Div(token::Slash { spans: [sp] }),
        BinOp::RemAssign(_) => BinOp::Rem(token::Percent { spans: [sp] }),
        _ => return None,
    })
}

fn binop(op: &BinOp, l: Value, r: Value) -> Result<Value> {
    use BinOp::*;
    Ok(match op {
        Add(_) | Sub(_) | Mul(_) | Div(_) | Rem(_) => arith(op.clone(), l, r)?,
        Eq(_) => Value::Bool(l.eq_value(&r)),
        Ne(_) => Value::Bool(!l.eq_value(&r)),
        Lt(_) | Le(_) | Gt(_) | Ge(_) => {
            let ord = compare(&l, &r)?;
            let b = match op {
                Lt(_) => ord == std::cmp::Ordering::Less,
                Le(_) => ord != std::cmp::Ordering::Greater,
                Gt(_) => ord == std::cmp::Ordering::Greater,
                Ge(_) => ord != std::cmp::Ordering::Less,
                _ => unreachable!(),
            };
            Value::Bool(b)
        }
        BitAnd(_) => int_bin(l, r, |a, b| a & b)?,
        BitOr(_) => int_bin(l, r, |a, b| a | b)?,
        BitXor(_) => int_bin(l, r, |a, b| a ^ b)?,
        Shl(_) => int_bin(l, r, |a, b| a << b)?,
        Shr(_) => int_bin(l, r, |a, b| a >> b)?,
        other => bail!("unsupported operator {:?}", other),
    })
}

fn arith(op: BinOp, l: Value, r: Value) -> Result<Value> {
    use BinOp::*;
    // String concatenation with +.
    if let (Add(_), Value::Str(a), Value::Str(b)) = (&op, &l, &r) {
        return Ok(Value::str(format!("{}{}", a.borrow(), b.borrow())));
    }
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
            Add(_) => a + b,
            Sub(_) => a - b,
            Mul(_) => a * b,
            Div(_) => {
                if b == 0 {
                    bail!("divide by zero");
                }
                a / b
            }
            Rem(_) => {
                if b == 0 {
                    bail!("remainder by zero");
                }
                a % b
            }
            _ => bail!("unsupported integer operator"),
        })),
        (a, b) => {
            let (x, y) = (to_float(&a)?, to_float(&b)?);
            Ok(Value::Float(match op {
                Add(_) => x + y,
                Sub(_) => x - y,
                Mul(_) => x * y,
                Div(_) => x / y,
                Rem(_) => x % y,
                _ => bail!("unsupported float operator"),
            }))
        }
    }
}

fn int_bin(l: Value, r: Value, f: impl Fn(i128, i128) -> i128) -> Result<Value> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(f(a, b))),
        (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(f(a as i128, b as i128) != 0)),
        _ => bail!("bitwise operators need integers"),
    }
}

fn compare(l: &Value, r: &Value) -> Result<std::cmp::Ordering> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            a.partial_cmp(b).ok_or_else(|| anyhow!("cannot order NaN"))?
        }
        (Value::Int(a), Value::Float(b)) => (*a as f64)
            .partial_cmp(b)
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Float(a), Value::Int(b)) => a
            .partial_cmp(&(*b as f64))
            .ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Str(a), Value::Str(b)) => a.borrow().cmp(&b.borrow()),
        (Value::Char(a), Value::Char(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (a, b) => bail!("cannot compare {} and {}", a.type_name(), b.type_name()),
    })
}

fn to_float(v: &Value) -> Result<f64> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => bail!("expected a number, got {}", other.type_name()),
    }
}

/// First concrete type argument of a path segment, `Vec<T>` gives `T`.
pub(super) fn first_generic_type(seg: &syn::PathSegment) -> Option<&syn::Type> {
    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                return Some(t);
            }
        }
    }
    None
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Infer(_) => "_ placeholder",
        Expr::Let(_) => "let expression",
        Expr::TryBlock(_) => "try block",
        Expr::Yield(_) => "yield",
        Expr::Const(_) => "const block",
        Expr::Verbatim(_) => "unparsed tokens",
        _ => "this expression",
    }
}

fn parse_vec_repeat(input: syn::parse::ParseStream) -> syn::Result<(Expr, Expr)> {
    let value: Expr = input.parse()?;
    input.parse::<syn::Token![;]>()?;
    let count: Expr = input.parse()?;
    Ok((value, count))
}
