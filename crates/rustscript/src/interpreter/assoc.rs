//! Associated functions like `String::from`, `Vec::new`, `File::open`,
//! `Duration::from_secs`.

use num_traits::AsPrimitive;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::int_methods::{from_bytes, from_bytes_order};
use super::jwt_bridge::jwt_assoc;
use super::native::Native;
use super::numeric::IntWidth;
use super::std_bridge::{
    arg_int, arg_str, as_i64, bytes_to_string, make_duration, make_path, open_file, path_like,
};
use super::value::Value;

/// Associated functions like `String::new`, `Vec::new`, `HashMap::new`.
pub(super) fn assoc_fn(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    if matches!(ty, "Header" | "EncodingKey") {
        return jwt_assoc(ty, func, args);
    }
    if let Some(v) = super::ratatui::ratatui_assoc(ty, func, args) {
        return Ok(Some(v));
    }
    // The groups use disjoint type and name pairs, so the first helper that
    // recognizes the pair answers.
    if let Some(v) = conversion_assoc(ty, func, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = container_assoc(ty, func, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = fs_process_assoc(ty, func, args)? {
        return Ok(Some(v));
    }
    misc_assoc(ty, func, args)
}

/// String, char, and numeric constructors and conversions.
fn conversion_assoc(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        ("String", "new" | "with_capacity") => Value::str(""),
        ("String", "from") => Value::str(args.first().map(Value::display).unwrap_or_default()),
        ("String", "from_utf8_lossy") => Value::str(bytes_to_string(args.first())),
        // `char::from` only converts a u8 in real Rust, so the byte range is
        // enforced even though every integer is an i64 here.
        ("char", "from") => match args.first() {
            Some(Value::Char(c)) => Value::Char(*c),
            Some(Value::Int(n)) => match u8::try_from(*n) {
                Ok(b) => Value::Char(char::from(b)),
                Err(_) => bail!("`char::from` needs a u8"),
            },
            _ => bail!("`char::from` needs a u8"),
        },
        ("char", "from_u32") => match args.first().and_then(as_i64) {
            Some(n) => match u32::try_from(n).ok().and_then(char::from_u32) {
                Some(c) => Value::some(Value::Char(c)),
                None => Value::none(),
            },
            _ => Value::none(),
        },
        ("char", "from_digit") => {
            let n = args.first().and_then(as_i64).unwrap_or(-1);
            let radix = args.get(1).and_then(as_i64).unwrap_or(10);
            match (u32::try_from(n), u32::try_from(radix)) {
                (Ok(n), Ok(radix)) => match char::from_digit(n, radix) {
                    Some(c) => Value::some(Value::Char(c)),
                    None => Value::none(),
                },
                _ => Value::none(),
            }
        }
        // Every integer type parses the same way here, values are untyped ints.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "from_str_radix",
        ) => {
            let text = args.first().map(Value::display).unwrap_or_default();
            let radix = args
                .get(1)
                .and_then(as_i64)
                .and_then(|r| u32::try_from(r).ok())
                .unwrap_or(10);
            match i64::from_str_radix(text.trim(), radix) {
                Ok(n) => Value::ok(Value::Int(n)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // Numeric `T::from(x)`. Every integer is an i64 here, so a widening
        // conversion just carries the value. `from` on a bool gives 0 or 1,
        // the same as `usize::from(cond)` and the like.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "from",
        ) => Value::Int(int_from_arg(ty, args.first())?),
        ("f32" | "f64", "from") => match args.first() {
            Some(Value::Float(f)) => Value::Float(*f),
            Some(Value::Int(n)) => Value::Float(AsPrimitive::<f64>::as_(*n)),
            Some(Value::Bool(b)) => Value::Float(if *b { 1.0 } else { 0.0 }),
            _ => bail!("`{ty}::from` needs a number"),
        },
        // Fallible `T::try_from(x)`. The value fits when it lands inside the
        // target range, so a narrowing conversion reports overflow with the
        // same message as the real `TryFromIntError`.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "try_from",
        ) => {
            let n = int_from_arg(ty, args.first())?;
            if int_fits(ty, n) {
                Value::ok(Value::Int(n))
            } else {
                Value::err(Value::str(
                    "out of range integral type conversion attempted",
                ))
            }
        }
        // `T::from_le_bytes` and its be and ne siblings. The result carries
        // the named width, so a `u32` read stays a u32 and an `i32` read of
        // the same four bytes is negative where the top bit is set.
        (
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize",
            "from_le_bytes" | "from_be_bytes" | "from_ne_bytes",
        ) => int_from_bytes(ty, func, args)?,
        ("String", "from_utf8") => Value::ok(Value::str(bytes_to_string(args.first()))),
        _ => return Ok(None),
    }))
}

/// Containers, wrappers, paths, regex, and the option and result names.
fn container_assoc(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        // The shape carries every field a later builder call can set, since a
        // shape cannot grow after the instance exists.
        ("Command", "new") => command_new(args.first().cloned().unwrap_or_else(|| Value::str(""))),
        ("Vec", "new" | "with_capacity") => Value::vec(vec![]),
        ("Vec", "from") => match args.first() {
            Some(Value::Vec(v)) => Value::vec(v.lock().clone()),
            Some(other) => Value::vec(vec![other.clone()]),
            None => Value::vec(vec![]),
        },
        ("HashMap" | "BTreeMap", "new") | ("HashMap", "with_capacity") => Value::map(),
        ("HashSet" | "BTreeSet", "new") | ("HashSet", "with_capacity") => Value::set(),
        ("Box" | "Rc" | "Arc" | "RefCell" | "Cell", "new") => {
            args.first().cloned().unwrap_or(Value::Unit)
        }
        // Our file and pipe readers are already buffered, so wrapping is a
        // pass-through; a raw socket is turned into a buffered reader.
        ("BufReader" | "BufWriter", "new" | "with_capacity") => match args.last() {
            Some(Value::Native(h)) if matches!(&*h.lock(), Native::Stream(_)) => {
                let cloned = {
                    let locked = h.lock();
                    let Native::Stream(s) = &*locked else {
                        unreachable!()
                    };
                    s.try_clone()
                };
                match cloned {
                    Ok(clone) => Native::Reader(std::io::BufReader::new(
                        Box::new(clone) as Box<dyn std::io::Read + Send>
                    ))
                    .wrap(),
                    Err(e) => return Err(anyhow!("cannot buffer socket: {e}")),
                }
            }
            other => other.cloned().unwrap_or(Value::Unit),
        },
        ("PathBuf", "new") => make_path(""),
        ("PathBuf" | "Path", "from") | ("Path", "new") => {
            make_path(args.first().map(path_like).unwrap_or_default())
        }
        ("Regex", "new") => {
            let pat = args.first().map(Value::display).unwrap_or_default();
            match regex::Regex::new(&pat) {
                Ok(compiled) => Value::ok(super::regex_bridge::make_regex(compiled, &pat)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        ("Some", _) | ("Option", "Some") => {
            Value::some(args.first().cloned().unwrap_or(Value::Unit))
        }
        ("Result", "Ok") => Value::ok(args.first().cloned().unwrap_or(Value::Unit)),
        ("Result", "Err") => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        _ => return Ok(None),
    }))
}

/// The `Command` builder shape shared by `Command::new` wherever it is spelled.
pub(super) fn command_new(program: Value) -> Value {
    Value::struct_of(
        "Command",
        [
            ("program".into(), program),
            ("args".into(), Value::vec(vec![])),
            ("cwd".into(), Value::Unit),
            ("envs".into(), Value::Unit),
            ("stdin".into(), Value::Unit),
            ("stdout".into(), Value::Unit),
            ("stderr".into(), Value::Unit),
        ],
    )
}

/// Files, permissions, and process stream markers.
fn fs_process_assoc(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        ("Permissions", "from_mode") => {
            let mode = args.first().and_then(as_i64).unwrap_or(0o644);
            Value::struct_of("Permissions", vec![("mode".into(), Value::Int(mode))])
        }
        // -- files -----------------------------------------------------
        ("File", "open") => open_file(&arg_str(args, 0), std::fs::OpenOptions::new().read(true)),
        ("File", "create") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        ),
        ("File", "create_new") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create_new(true),
        ),
        ("OpenOptions", "new") => Value::struct_of(
            "OpenOptions",
            [
                "read",
                "write",
                "append",
                "create",
                "create_new",
                "truncate",
            ]
            .into_iter()
            .map(|k| (Arc::from(k), Value::Bool(false))),
        ),
        ("Stdio", "piped" | "inherit" | "null") => {
            Value::struct_of("Stdio", [("kind".into(), Value::str(func))])
        }
        // `Stdio::from(file)` sends a child's stream straight to an open file.
        // The marker carries the file, and the handle is cloned when the
        // command is built so the script keeps its own copy.
        ("Stdio", "from") => {
            let Some(file @ Value::Native(_)) = args.first() else {
                bail!(
                    "Stdio::from takes an open File, got {}",
                    args.first().map_or("nothing", Value::type_name)
                );
            };
            Value::struct_of(
                "Stdio",
                [
                    ("kind".into(), Value::str("file")),
                    ("file".into(), file.clone()),
                ],
            )
        }
        _ => return Ok(None),
    }))
}

/// Time, net, pdf, xml, and the seek positions.
fn misc_assoc(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        // -- time ------------------------------------------------------
        ("Instant", "now") => Native::Instant(std::time::Instant::now()).wrap(),
        ("SystemTime", "now") => Native::SystemTime(std::time::SystemTime::now()).wrap(),
        ("Duration", "from_secs") => make_duration(std::time::Duration::from_secs(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        ("Duration", "from_millis") => make_duration(std::time::Duration::from_millis(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        ("Duration", "from_micros") => make_duration(std::time::Duration::from_micros(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        ("Duration", "from_nanos") => make_duration(std::time::Duration::from_nanos(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        ("Duration", "new") => make_duration(std::time::Duration::new(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
            u32::try_from(arg_int(args, 1)).unwrap_or_default(),
        )),
        // -- net -------------------------------------------------------
        ("TcpListener", "bind") => match std::net::TcpListener::bind(arg_str(args, 0)) {
            Ok(l) => Value::ok(Native::Listener(l).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("TcpStream", "connect") => match std::net::TcpStream::connect(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Stream(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("UdpSocket", "bind") => match std::net::UdpSocket::bind(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Udp(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("Document", "load") => super::pdf_bridge::load(&arg_str(args, 0)),
        ("Element", "parse") => super::xmltree_bridge::parse(args),
        ("Element", "new") => super::xmltree_bridge::new_element(&arg_str(args, 0)),
        // The real xmltree node enum, constructed like `SeekFrom` below since
        // no user declaration exists for it.
        ("XMLNode", "Element" | "Text" | "Comment" | "CData" | "ProcessingInstruction") => {
            Value::Enum {
                enum_name: Arc::from("XMLNode"),
                variant: Arc::from(func),
                data: args.iter().cloned().collect(),
            }
        }
        ("SeekFrom", "Start" | "End" | "Current") => Value::Enum {
            enum_name: Arc::from("SeekFrom"),
            variant: Arc::from(func),
            data: Arc::from(vec![args.first().cloned().unwrap_or(Value::Int(0))]),
        },
        _ => return Ok(None),
    }))
}

/// Pull an integer out of a `from`/`try_from` argument. Ints carry through,
/// a bool becomes 0 or 1, and a char becomes its scalar value.
fn int_from_arg(ty: &str, v: Option<&Value>) -> Result<i64> {
    match v {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Bool(b)) => Ok(i64::from(*b)),
        Some(Value::Char(c)) => Ok(*c as i64),
        _ => bail!("`{ty}` conversion needs an integer"),
    }
}

/// `T::from_le_bytes([..])` and its be and ne siblings, over the same shared
/// core the `to_*_bytes` methods use.
fn int_from_bytes(ty: &str, func: &str, args: &[Value]) -> Result<Value> {
    let (Some(width), Some(order)) = (IntWidth::parse(ty), from_bytes_order(func)) else {
        bail!("`{ty}::{func}` is not a byte conversion");
    };
    let bytes = byte_array(ty, func, args.first())?;
    Ok(Value::int_of_width(
        from_bytes(width, order, &bytes)?,
        width,
    ))
}

/// The `[u8; N]` argument of a byte conversion. An array literal is a vec at
/// runtime, so the shape real Rust guarantees in its type is read back here.
fn byte_array(ty: &str, func: &str, arg: Option<&Value>) -> Result<Vec<i128>> {
    let Some(Value::Vec(items)) = arg else {
        bail!("`{ty}::{func}` needs a byte array");
    };
    let items = items.lock();
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter() {
        let Some((value, _)) = item.int_parts() else {
            bail!("`{ty}::{func}` needs a byte array");
        };
        out.push(value);
    }
    Ok(out)
}

/// Whether `n` lands inside the target integer type range.
fn int_fits(ty: &str, n: i64) -> bool {
    match ty {
        "i8" => i8::try_from(n).is_ok(),
        "i16" => i16::try_from(n).is_ok(),
        "i32" => i32::try_from(n).is_ok(),
        "u8" => u8::try_from(n).is_ok(),
        "u16" => u16::try_from(n).is_ok(),
        "u32" => u32::try_from(n).is_ok(),
        "u64" | "u128" | "usize" => n >= 0,
        _ => true,
    }
}
