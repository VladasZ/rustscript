//! Enum definitions shared by the compiler, the VM, and the bridges. An enum
//! value points at its definition and carries a variant index, so a variant
//! test is a pointer compare and an int compare. The names live in the
//! definition and are read only for printing and for the dynamic paths that
//! still arrive with a name.

use std::sync::{Arc, LazyLock};

use super::bytecode::NO_TYPE;

/// The enums the interpreter itself gives meaning to. Everything else, user
/// enums and bridge enums alike, is `Other` and is inspected only through its
/// definition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnumKind {
    Option,
    Result,
    Ordering,
    VarError,
    Other,
}

pub struct VariantDef {
    pub name: Arc<str>,
    pub unit: bool,
}

pub struct EnumDef {
    pub kind: EnumKind,
    /// Declared by the script, as opposed to a builtin the interpreter
    /// or a bridge provides.
    pub user: bool,
    /// The canonical name, `crate::Shape` for a user enum and the bare type
    /// name for a builtin.
    pub name: Arc<str>,
    /// The index into the impl table for a user enum, `NO_TYPE` for a
    /// builtin.
    pub type_id: u16,
    pub variants: Vec<VariantDef>,
}

impl EnumDef {
    pub fn new(
        kind: EnumKind,
        name: impl Into<Arc<str>>,
        type_id: u16,
        variants: impl IntoIterator<Item = (Arc<str>, bool)>,
    ) -> Arc<EnumDef> {
        EnumDef::build(kind, true, name, type_id, variants)
    }

    fn build(
        kind: EnumKind,
        user: bool,
        name: impl Into<Arc<str>>,
        type_id: u16,
        variants: impl IntoIterator<Item = (Arc<str>, bool)>,
    ) -> Arc<EnumDef> {
        Arc::new(EnumDef {
            kind,
            user,
            name: name.into(),
            type_id,
            variants: variants
                .into_iter()
                .map(|(name, unit)| VariantDef { name, unit })
                .collect(),
        })
    }

    fn units(kind: EnumKind, name: &str, variants: &[&str]) -> Arc<EnumDef> {
        EnumDef::build(
            kind,
            false,
            name,
            NO_TYPE,
            variants.iter().map(|v| (Arc::from(*v), true)),
        )
    }

    fn tuples(kind: EnumKind, name: &str, variants: &[(&str, bool)]) -> Arc<EnumDef> {
        EnumDef::build(
            kind,
            false,
            name,
            NO_TYPE,
            variants.iter().map(|(v, unit)| (Arc::from(*v), *unit)),
        )
    }

    pub fn variant_index(&self, name: &str) -> Option<u16> {
        self.variants
            .iter()
            .position(|v| &*v.name == name)
            .and_then(|i| u16::try_from(i).ok())
    }

    pub fn variant_name(&self, index: u16) -> &Arc<str> {
        &self.variants[usize::from(index)].name
    }

    pub fn is_unit(&self, index: u16) -> bool {
        self.variants[usize::from(index)].unit
    }

    /// Whether two values share a definition. Definitions are created once
    /// per enum, so identity is the test.
    pub fn same(a: &Arc<EnumDef>, b: &Arc<EnumDef>) -> bool {
        Arc::ptr_eq(a, b)
    }
}

pub const NONE: u16 = 0;
pub const SOME: u16 = 1;
pub const OK: u16 = 0;
pub const ERR: u16 = 1;
pub const LESS: u16 = 0;
pub const EQUAL: u16 = 1;
pub const GREATER: u16 = 2;
pub const NOT_PRESENT: u16 = 0;
pub const NOT_UNICODE: u16 = 1;

// Variant order follows the std declaration, so the derived `PartialOrd`
// of real Rust falls out of the index compare.
pub static OPTION: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::Option,
        "Option",
        &[("None", true), ("Some", false)],
    )
});

pub static RESULT: LazyLock<Arc<EnumDef>> =
    LazyLock::new(|| EnumDef::tuples(EnumKind::Result, "Result", &[("Ok", false), ("Err", false)]));

pub static ORDERING: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Ordering,
        "Ordering",
        &["Less", "Equal", "Greater"],
    )
});

pub static VAR_ERROR: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::VarError,
        "VarError",
        &[("NotPresent", true), ("NotUnicode", false)],
    )
});

/// Every name `{:?}` of a `std::io::ErrorKind` can print, stable and
/// unstable, since `io_error_value` formats the real kind.
pub static ERROR_KIND: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "ErrorKind",
        &[
            "NotFound",
            "PermissionDenied",
            "ConnectionRefused",
            "ConnectionReset",
            "HostUnreachable",
            "NetworkUnreachable",
            "ConnectionAborted",
            "NotConnected",
            "AddrInUse",
            "AddrNotAvailable",
            "NetworkDown",
            "BrokenPipe",
            "AlreadyExists",
            "WouldBlock",
            "NotADirectory",
            "IsADirectory",
            "DirectoryNotEmpty",
            "ReadOnlyFilesystem",
            "FilesystemLoop",
            "StaleNetworkFileHandle",
            "InvalidInput",
            "InvalidData",
            "TimedOut",
            "WriteZero",
            "StorageFull",
            "NotSeekable",
            "QuotaExceeded",
            "FileTooLarge",
            "ResourceBusy",
            "ExecutableFileBusy",
            "Deadlock",
            "CrossesDevices",
            "TooManyLinks",
            "InvalidFilename",
            "ArgumentListTooLong",
            "Interrupted",
            "Unsupported",
            "UnexpectedEof",
            "OutOfMemory",
            "InProgress",
            "Other",
            "Uncategorized",
        ],
    )
});

pub static SEEK_FROM: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::Other,
        "SeekFrom",
        &[("Start", false), ("End", false), ("Current", false)],
    )
});

pub static XML_NODE: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::Other,
        "XMLNode",
        &[
            ("Element", false),
            ("Comment", false),
            ("CData", false),
            ("Text", false),
            ("ProcessingInstruction", false),
        ],
    )
});

pub static COLOR: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::Other,
        "Color",
        &[
            ("Reset", true),
            ("Black", true),
            ("Red", true),
            ("Green", true),
            ("Yellow", true),
            ("Blue", true),
            ("Magenta", true),
            ("Cyan", true),
            ("Gray", true),
            ("DarkGray", true),
            ("LightRed", true),
            ("LightGreen", true),
            ("LightYellow", true),
            ("LightBlue", true),
            ("LightMagenta", true),
            ("LightCyan", true),
            ("White", true),
            ("Rgb", false),
            ("Indexed", false),
        ],
    )
});

pub static CONSTRAINT: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::tuples(
        EnumKind::Other,
        "Constraint",
        &[
            ("Min", false),
            ("Max", false),
            ("Length", false),
            ("Percentage", false),
            ("Ratio", false),
            ("Fill", false),
        ],
    )
});

pub static BORDER_TYPE: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "BorderType",
        &[
            "Plain",
            "Rounded",
            "Double",
            "Thick",
            "QuadrantInside",
            "QuadrantOutside",
        ],
    )
});

pub static ALGORITHM: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "Algorithm",
        &[
            "HS256", "HS384", "HS512", "ES256", "ES384", "RS256", "RS384", "RS512", "PS256",
            "PS384", "PS512", "EdDSA",
        ],
    )
});

pub static SERVICE_STATE: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "ServiceState",
        &[
            "Stopped",
            "StartPending",
            "StopPending",
            "Running",
            "ContinuePending",
            "PausePending",
            "Paused",
        ],
    )
});

pub static SERVICE_START_TYPE: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "ServiceStartType",
        &[
            "AutoStart",
            "OnDemand",
            "Disabled",
            "BootStart",
            "SystemStart",
        ],
    )
});

pub static REG_TYPE: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "RegType",
        &[
            "REG_NONE",
            "REG_SZ",
            "REG_EXPAND_SZ",
            "REG_BINARY",
            "REG_DWORD",
            "REG_MULTI_SZ",
            "REG_QWORD",
        ],
    )
});

pub static REG_DISPOSITION: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    EnumDef::units(
        EnumKind::Other,
        "RegDisposition",
        &["REG_CREATED_NEW_KEY", "REG_OPENED_EXISTING_KEY"],
    )
});

/// A builtin enum by its bare type name, for `Ordering::Less` style paths
/// and patterns that no user declaration covers.
pub fn builtin_enum(name: &str) -> Option<&'static Arc<EnumDef>> {
    Some(match name {
        "RegDisposition" => &REG_DISPOSITION,
        "Option" => &OPTION,
        "Result" => &RESULT,
        "Ordering" => &ORDERING,
        "VarError" => &VAR_ERROR,
        "ErrorKind" => &ERROR_KIND,
        "SeekFrom" => &SEEK_FROM,
        "XMLNode" => &XML_NODE,
        "Color" => &COLOR,
        "Constraint" => &CONSTRAINT,
        "BorderType" => &BORDER_TYPE,
        "Algorithm" => &ALGORITHM,
        "ServiceState" => &SERVICE_STATE,
        "ServiceStartType" => &SERVICE_START_TYPE,
        "RegType" => &REG_TYPE,
        _ => return None,
    })
}

/// The variants the prelude brings in bare, `Some` without `Option::`.
pub fn prelude_variant(name: &str) -> Option<(&'static Arc<EnumDef>, u16)> {
    Some(match name {
        "None" => (&OPTION, NONE),
        "Some" => (&OPTION, SOME),
        "Ok" => (&RESULT, OK),
        "Err" => (&RESULT, ERR),
        _ => return None,
    })
}
