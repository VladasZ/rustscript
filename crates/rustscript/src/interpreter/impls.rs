//! The script's own `impl` methods, found by type id and method id instead
//! of by name. Every user struct and enum gets a `type_id` at load, carried
//! on its shape or enum definition, and every method name is either a
//! `BuiltinId` or an atom interned at compile time, so a method call on a
//! user value is two integer lookups with no string built.

use std::collections::HashMap;
use std::sync::Arc;

use super::bytecode::{BinKind, BuiltinId, Chunk, MethodName, NO_ATOM, NO_TYPE, UnKind};
use super::value::Value;

/// The methods one user type declares.
#[derive(Default)]
pub struct TypeMethods {
    /// Methods whose name is a bridge method name too, `len`, `next`,
    /// `clone`, sorted by id for a binary search.
    by_builtin: Vec<(BuiltinId, Arc<Chunk>)>,
    /// Methods with a script only name, sorted by atom.
    by_atom: Vec<(u32, Arc<Chunk>)>,
    pub display: Option<Arc<Chunk>>,
    pub debug: Option<Arc<Chunk>>,
    pub drop: Option<Arc<Chunk>>,
    pub next: Option<Arc<Chunk>>,
    /// The operator trait impls, `Add::add` and friends, by `bin_slot`.
    bin: [Option<Arc<Chunk>>; BIN_SLOTS],
    /// The assigning forms, `AddAssign::add_assign`, by `bin_slot`.
    bin_assign: [Option<Arc<Chunk>>; BIN_SLOTS],
    pub neg: Option<Arc<Chunk>>,
    pub not: Option<Arc<Chunk>>,
}

const BIN_SLOTS: usize = 10;

/// The operator trait method names, in `bin_slot` order.
const BIN_NAMES: [&str; BIN_SLOTS] = [
    "add", "sub", "mul", "div", "rem", "bitand", "bitor", "bitxor", "shl", "shr",
];

/// The assigning operator names, same order as `BIN_NAMES`.
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

/// The slot of an operator with a trait impl, None for the comparisons,
/// which answer through the derived semantics.
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
    /// The method a call site names, by builtin id or by atom.
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

/// The atoms of the method names no bridge knows, fixed before compiling so
/// every call site can carry its atom.
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

/// Every impl method of the program, by type id.
pub struct ImplTable {
    types: Vec<TypeMethods>,
    /// Type name to id, for the cold lookups that start from a name: a path
    /// call on a user type, a tuple struct built at runtime, and a user impl
    /// on a bridge type name, whose values carry no id.
    type_ids: HashMap<Arc<str>, u16>,
    atoms: HashMap<String, u32>,
    /// Whether some impl targets a type the script did not declare, `impl
    /// MyTrait for PathBuf`. Only then does a bridge value look up its
    /// methods by name.
    foreign: bool,
    /// Every `(type, method)` pair, for the coverage report.
    names: Vec<(String, String)>,
}

impl ImplTable {
    /// `methods` are the compiled impl methods, `type_ids` the ids the
    /// resolver handed out, and `declared` the names of the script's own
    /// structs and enums.
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

    /// True when the script declares no impl at all, which lets the hot
    /// paths skip every user lookup.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.names.iter().cloned()
    }

    /// The methods of the value's type, None for a value of no user type.
    pub fn of_value(&self, value: &Value) -> Option<&TypeMethods> {
        let (type_id, name) = match value {
            Value::Struct(s) => (s.shape.type_id, &s.shape.name),
            Value::Enum { def, .. } => (def.type_id, &def.name),
            _ => return None,
        };
        if type_id != NO_TYPE {
            return self.types.get(usize::from(type_id));
        }
        if !self.foreign {
            return None;
        }
        self.of_name(name)
    }

    /// The methods of a type named at runtime, a path call's namespace.
    pub fn of_name(&self, ty: &str) -> Option<&TypeMethods> {
        let id = *self.type_ids.get(ty)?;
        self.types.get(usize::from(id))
    }

    /// The id of a type named at runtime, to tag a value built by name.
    pub fn type_id(&self, ty: &str) -> u16 {
        self.type_ids.get(ty).copied().unwrap_or(NO_TYPE)
    }

    /// A method named at runtime, `Type::method` in a path call.
    pub fn by_name(&self, ty: &str, method: &str) -> Option<Arc<Chunk>> {
        let methods = self.of_name(ty)?;
        let name = self.method_name(method);
        methods.get(&name).cloned()
    }

    /// A call site name built at runtime, with its atom filled in.
    pub fn method_name(&self, method: &str) -> MethodName {
        MethodName {
            id: BuiltinId::resolve(method),
            atom: self.atoms.get(method).copied().unwrap_or(NO_ATOM),
            text: method.to_string(),
            scalar: None,
        }
    }
}
