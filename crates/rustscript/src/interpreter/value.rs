use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::rc::Rc;

/// A runtime value. Containers use `Rc<RefCell<..>>` so that `&mut` aliasing and
/// shared mutation behave, since the interpreter ignores ownership entirely.
#[derive(Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i128),
    Float(f64),
    Char(char),
    Str(Rc<RefCell<String>>),
    Vec(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<BTreeMap<MapKey, Value>>>),
    Tuple(Rc<RefCell<Vec<Value>>>),
    /// Struct instance. Named fields, or positional for tuple structs.
    Struct {
        name: String,
        fields: Rc<RefCell<BTreeMap<String, Value>>>,
    },
    /// Enum value, including the builtin Option and Result.
    Enum {
        enum_name: String,
        variant: String,
        data: Rc<RefCell<Vec<Value>>>,
    },
    Range {
        start: i128,
        end: i128,
        inclusive: bool,
    },
    Closure(Rc<ClosureData>),
}

/// A closure captures the variables visible where it was written, by value.
/// Container captures share their `Rc`, so mutation through them is visible.
pub struct ClosureData {
    pub params: Vec<syn::Pat>,
    pub body: syn::Expr,
    pub captured: std::collections::HashMap<String, Value>,
}

/// Hashable/orderable key for maps. Only a subset of values can be keys.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapKey {
    Bool(bool),
    Int(i128),
    Char(char),
    Str(String),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(Rc::new(RefCell::new(s.into())))
    }

    pub fn vec(items: Vec<Value>) -> Value {
        Value::Vec(Rc::new(RefCell::new(items)))
    }

    pub fn some(v: Value) -> Value {
        Value::Enum {
            enum_name: "Option".into(),
            variant: "Some".into(),
            data: Rc::new(RefCell::new(vec![v])),
        }
    }

    pub fn none() -> Value {
        Value::Enum {
            enum_name: "Option".into(),
            variant: "None".into(),
            data: Rc::new(RefCell::new(vec![])),
        }
    }

    pub fn ok(v: Value) -> Value {
        Value::Enum {
            enum_name: "Result".into(),
            variant: "Ok".into(),
            data: Rc::new(RefCell::new(vec![v])),
        }
    }

    pub fn err(v: Value) -> Value {
        Value::Enum {
            enum_name: "Result".into(),
            variant: "Err".into(),
            data: Rc::new(RefCell::new(vec![v])),
        }
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Char(_) => "char",
            Value::Str(_) => "String",
            Value::Vec(_) => "Vec",
            Value::Map(_) => "HashMap",
            Value::Tuple(_) => "tuple",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Range { .. } => "range",
            Value::Closure(_) => "closure",
        }
    }

    pub fn as_key(&self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Int(i) => MapKey::Int(*i),
            Value::Char(c) => MapKey::Char(*c),
            Value::Str(s) => MapKey::Str(s.borrow().clone()),
            _ => return None,
        })
    }

    /// Value equality used by `==`, `match`, and map lookups.
    pub fn eq_value(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => *a as f64 == *b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => *a.borrow() == *b.borrow(),
            (Value::Vec(a), Value::Vec(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_value(y))
            }
            (
                Value::Enum {
                    enum_name: ea,
                    variant: va,
                    data: da,
                },
                Value::Enum {
                    enum_name: eb,
                    variant: vb,
                    data: db,
                },
            ) => {
                ea == eb && va == vb && {
                    let (da, db) = (da.borrow(), db.borrow());
                    da.len() == db.len() && da.iter().zip(db.iter()).all(|(x, y)| x.eq_value(y))
                }
            }
            (
                Value::Struct {
                    name: na,
                    fields: fa,
                },
                Value::Struct {
                    name: nb,
                    fields: fb,
                },
            ) => {
                na == nb && {
                    let (fa, fb) = (fa.borrow(), fb.borrow());
                    fa.len() == fb.len()
                        && fa
                            .iter()
                            .all(|(k, v)| fb.get(k).map(|o| v.eq_value(o)).unwrap_or(false))
                }
            }
            _ => false,
        }
    }

    /// `Display`, the `{}` format.
    pub fn display(&self) -> String {
        match self {
            Value::Unit => "()".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Char(c) => c.to_string(),
            Value::Str(s) => s.borrow().clone(),
            other => other.debug(),
        }
    }

    /// `Debug`, the `{:?}` format.
    pub fn debug(&self) -> String {
        let mut out = String::new();
        self.write_debug(&mut out);
        out
    }

    fn write_debug(&self, out: &mut String) {
        match self {
            Value::Unit => out.push_str("()"),
            Value::Bool(b) => write!(out, "{b}").unwrap(),
            Value::Int(i) => write!(out, "{i}").unwrap(),
            Value::Float(f) => out.push_str(&format_float(*f)),
            Value::Char(c) => write!(out, "{c:?}").unwrap(),
            Value::Str(s) => write!(out, "{:?}", s.borrow()).unwrap(),
            Value::Range {
                start,
                end,
                inclusive,
            } => {
                let sep = if *inclusive { "..=" } else { ".." };
                write!(out, "{start}{sep}{end}").unwrap();
            }
            Value::Vec(items) => {
                out.push('[');
                for (i, v) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                out.push(']');
            }
            Value::Tuple(items) => {
                out.push('(');
                let items = items.borrow();
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                if items.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            Value::Map(map) => {
                out.push('{');
                for (i, (k, v)) in map.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_debug(out);
                    out.push_str(": ");
                    v.write_debug(out);
                }
                out.push('}');
            }
            Value::Struct { name, fields } => {
                write!(out, "{name}").unwrap();
                let fields = fields.borrow();
                if !fields.is_empty() {
                    out.push_str(" { ");
                    for (i, (k, v)) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        write!(out, "{k}: ").unwrap();
                        v.write_debug(out);
                    }
                    out.push_str(" }");
                }
            }
            Value::Closure(_) => out.push_str("<closure>"),
            Value::Enum {
                variant, data, ..
            } => {
                write!(out, "{variant}").unwrap();
                let data = data.borrow();
                if !data.is_empty() {
                    out.push('(');
                    for (i, v) in data.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        v.write_debug(out);
                    }
                    out.push(')');
                }
            }
        }
    }
}

impl MapKey {
    fn write_debug(&self, out: &mut String) {
        match self {
            MapKey::Bool(b) => write!(out, "{b}").unwrap(),
            MapKey::Int(i) => write!(out, "{i}").unwrap(),
            MapKey::Char(c) => write!(out, "{c:?}").unwrap(),
            MapKey::Str(s) => write!(out, "{s:?}").unwrap(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Int(i) => Value::Int(*i),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Str(s) => Value::str(s.clone()),
        }
    }
}

fn format_float(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}
