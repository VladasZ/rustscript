//! The script's own `impl` methods, found by type id and method id, so a
//! call on a user value is 2 integer lookups with no string built.

use std::collections::HashMap;
use std::sync::Arc;

use super::bytecode::{BinKind, BuiltinId, Chunk, MethodName, NO_ATOM, NO_TYPE, ScalarTy, UnKind};
use super::numeric::IntWidth;
use super::value::Value;

#[derive(Default)]
pub struct TypeMethods {
    /// Names a bridge knows too, sorted by id for a binary search.
    by_builtin: Vec<(BuiltinId, Arc<Chunk>)>,
    /// Script only names, sorted by atom.
    by_atom: Vec<(u32, Arc<Chunk>)>,
    pub display: Option<Arc<Chunk>>,
    pub debug: Option<Arc<Chunk>>,
    pub drop: Option<Arc<Chunk>>,
    pub next: Option<Arc<Chunk>>,
    /// `Add::add` and friends, by `bin_slot`.
    bin: [Option<Arc<Chunk>>; BIN_SLOTS],
    /// `AddAssign::add_assign` and friends, by `bin_slot`.
    bin_assign: [Option<Arc<Chunk>>; BIN_SLOTS],
    pub neg: Option<Arc<Chunk>>,
    pub not: Option<Arc<Chunk>>,
}

const BIN_SLOTS: usize = 10;

/// In `bin_slot` order.
const BIN_NAMES: [&str; BIN_SLOTS] = [
    "add", "sub", "mul", "div", "rem", "bitand", "bitor", "bitxor", "shl", "shr",
];

/// Same order as `BIN_NAMES`.
const BIN_ASSIGN_NAMES: [&str; BIN_SLOTS] = [
    "add_assign",
    "sub_assign",
    "mul_assign",
    "div_assign",
    "rem_assign",
    "bitand_assign",
    "bitor_assign",
    "bitxor_assign",
    "shl_assign",
    "shr_assign",
];

/// None for the comparisons, which answer through the derived semantics.
fn bin_slot(op: BinKind) -> Option<usize> {
    Some(match op {
        BinKind::Add => 0,
        BinKind::Sub => 1,
        BinKind::Mul => 2,
        BinKind::Div => 3,
        BinKind::Rem => 4,
        BinKind::BitAnd => 5,
        BinKind::BitOr => 6,
        BinKind::BitXor => 7,
        BinKind::Shl => 8,
        BinKind::Shr => 9,
        BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge => {
            return None;
        }
    })
}

impl TypeMethods {
    pub fn get(&self, name: &MethodName) -> Option<&Arc<Chunk>> {
        if name.id == BuiltinId::Other {
            if name.atom == NO_ATOM {
                return None;
            }
            self.by_atom
                .binary_search_by_key(&name.atom, |(atom, _)| *atom)
                .ok()
                .map(|index| &self.by_atom[index].1)
        } else {
            self.by_builtin
                .binary_search_by_key(&(name.id as u16), |(id, _)| *id as u16)
                .ok()
                .map(|index| &self.by_builtin[index].1)
        }
    }

    pub fn bin(&self, op: BinKind) -> Option<&Arc<Chunk>> {
        self.bin[bin_slot(op)?].as_ref()
    }

    pub fn bin_assign(&self, op: BinKind) -> Option<&Arc<Chunk>> {
        self.bin_assign[bin_slot(op)?].as_ref()
    }

    pub fn un(&self, op: UnKind) -> Option<&Arc<Chunk>> {
        match op {
            UnKind::Neg => self.neg.as_ref(),
            UnKind::Not => self.not.as_ref(),
        }
    }
}

/// Fixed before compiling so every call site can carry its atom.
pub fn method_atoms<'a>(names: impl IntoIterator<Item = &'a str>) -> HashMap<String, u32> {
    let mut atoms: HashMap<String, u32> = HashMap::new();
    for name in names {
        if BuiltinId::resolve(name) == BuiltinId::Other && !atoms.contains_key(name) {
            let next = u32::try_from(atoms.len()).expect("atom count fits u32");
            atoms.insert(name.to_string(), next);
        }
    }
    atoms
}

pub struct ImplTable {
    types: Vec<TypeMethods>,
    /// For the cold lookups that start from a name.
    type_ids: HashMap<Arc<str>, u16>,
    atoms: HashMap<String, u32>,
    /// Some impl targets a type the script did not declare, like
    /// `impl MyTrait for PathBuf`. Only then does a bridge value look up its
    /// methods.
    foreign: bool,
    /// For the coverage report.
    names: Vec<(String, String)>,
}

impl ImplTable {
    pub fn build(
        methods: Vec<(String, String, Arc<Chunk>)>,
        type_ids: HashMap<Arc<str>, u16>,
        atoms: HashMap<String, u32>,
        declared: &impl Fn(&str) -> bool,
    ) -> ImplTable {
        let count = type_ids.values().map(|id| usize::from(*id) + 1).max();
        let mut types: Vec<TypeMethods> = Vec::new();
        types.resize_with(count.unwrap_or(0), TypeMethods::default);
        let mut foreign = false;
        let mut names = Vec::with_capacity(methods.len());
        for (ty, name, chunk) in methods {
            let id = type_ids[ty.as_str()];
            foreign |= !declared(&ty);
            let entry = &mut types[usize::from(id)];
            match name.as_str() {
                "Display::fmt" => entry.display = Some(chunk.clone()),
                "Debug::fmt" => entry.debug = Some(chunk.clone()),
                "Drop::drop" => entry.drop = Some(chunk.clone()),
                "next" => entry.next = Some(chunk.clone()),
                "neg" => entry.neg = Some(chunk.clone()),
                "not" => entry.not = Some(chunk.clone()),
                other => {
                    if let Some(slot) = BIN_NAMES.iter().position(|n| *n == other) {
                        entry.bin[slot] = Some(chunk.clone());
                    } else if let Some(slot) = BIN_ASSIGN_NAMES.iter().position(|n| *n == other) {
                        entry.bin_assign[slot] = Some(chunk.clone());
                    }
                }
            }
            match BuiltinId::resolve(&name) {
                BuiltinId::Other => {
                    let atom = atoms[name.as_str()];
                    entry.by_atom.push((atom, chunk));
                }
                id => entry.by_builtin.push((id, chunk)),
            }
            names.push((ty, name));
        }
        for entry in &mut types {
            entry.by_builtin.sort_by_key(|(id, _)| *id as u16);
            entry.by_atom.sort_by_key(|(atom, _)| *atom);
        }
        ImplTable {
            types,
            type_ids,
            atoms,
            foreign,
            names,
        }
    }

    /// Lets the hot paths skip every user lookup.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.names.iter().cloned()
    }

    pub fn of_value(&self, value: &Value) -> Option<&TypeMethods> {
        self.of_receiver(value, None)
    }

    /// An empty vec has no element to name its type, so the written
    /// `Vec<u8>` picks its impl.
    pub fn of_receiver(&self, value: &Value, written: Option<&ScalarTy>) -> Option<&TypeMethods> {
        let (type_id, name) = match value {
            Value::Struct(s) => (s.shape.type_id, &s.shape.name),
            Value::Enum { def, .. } => (def.type_id, &def.name),
            // `impl Describe for Vec<String>` only exists when some impl
            // targets an undeclared type.
            other => {
                return self
                    .foreign
                    .then(|| self.of_builtin(other, written))
                    .flatten();
            }
        };
        if type_id != NO_TYPE {
            return self.types.get(usize::from(type_id));
        }
        if !self.foreign {
            return None;
        }
        self.of_name(name)
    }

    /// Keyed the way `impl_target` wrote it. An empty vec falls back to the
    /// one `Vec<..>` impl when there is exactly one, an untagged integer to
    /// the one integer impl.
    fn of_builtin(&self, value: &Value, written: Option<&ScalarTy>) -> Option<&TypeMethods> {
        match value {
            Value::Vec(items) => {
                let exact = items
                    .lock()
                    .first()
                    .and_then(builtin_key)
                    .and_then(|elem| self.of_name(&format!("Vec<{elem}>")));
                exact
                    .or_else(|| {
                        written
                            .and_then(written_key)
                            .and_then(|key| self.of_name(&key))
                    })
                    .or_else(|| self.of_unique(|name| name.starts_with("Vec<")))
            }
            Value::Int(_) => self
                .of_name("i64")
                .or_else(|| self.of_unique(|name| IntWidth::parse(name).is_some())),
            other => self.of_name(scalar_key(other)?),
        }
    }

    /// None when none or several match.
    fn of_unique(&self, pick: impl Fn(&str) -> bool) -> Option<&TypeMethods> {
        let mut found = self.type_ids.iter().filter(|(name, _)| pick(name));
        let (_, id) = found.next()?;
        if found.next().is_some() {
            return None;
        }
        self.types.get(usize::from(*id))
    }

    pub fn of_name(&self, ty: &str) -> Option<&TypeMethods> {
        let id = *self.type_ids.get(ty)?;
        self.types.get(usize::from(id))
    }

    pub fn type_id(&self, ty: &str) -> u16 {
        self.type_ids.get(ty).copied().unwrap_or(NO_TYPE)
    }

    pub fn by_name(&self, ty: &str, method: &str) -> Option<Arc<Chunk>> {
        let methods = self.of_name(ty)?;
        let name = self.method_name(method);
        methods.get(&name).cloned()
    }

    pub fn method_name(&self, method: &str) -> MethodName {
        MethodName {
            id: BuiltinId::resolve(method),
            atom: self.atoms.get(method).copied().unwrap_or(NO_ATOM),
            text: method.to_string(),
            scalar: None,
            default: None,
            place: false,
        }
    }
}

fn scalar_key(value: &Value) -> Option<&'static str> {
    Some(match value {
        Value::Bool(_) => "bool",
        Value::Int(_) => "i64",
        Value::IntW(_, width) | Value::Big(_, width) => width.name(),
        Value::Float(_) => "f64",
        Value::F32(_) => "f32",
        Value::Char(_) => "char",
        Value::Str(_) => "String",
        _ => return None,
    })
}

fn builtin_key(value: &Value) -> Option<String> {
    if let Some(key) = scalar_key(value) {
        return Some(key.to_string());
    }
    let Value::Vec(items) = value else {
        return None;
    };
    let elem = items.lock().first().and_then(builtin_key)?;
    Some(format!("Vec<{elem}>"))
}

/// The same form `builtin_key` rebuilds from a value.
fn written_key(ty: &ScalarTy) -> Option<String> {
    Some(match ty {
        ScalarTy::Int(width) => width.name().to_string(),
        ScalarTy::F32 => "f32".to_string(),
        ScalarTy::F64 => "f64".to_string(),
        ScalarTy::Bool => "bool".to_string(),
        ScalarTy::Char => "char".to_string(),
        ScalarTy::Str => "String".to_string(),
        ScalarTy::List(elem) => format!("Vec<{}>", written_key(elem)?),
        ScalarTy::Opt(_) | ScalarTy::Map(_) | ScalarTy::Set(_) | ScalarTy::Other => return None,
    })
}
