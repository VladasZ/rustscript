//! Associated functions like `String::from`, `Vec::new`, `File::open`,
//! `Duration::from_secs`.

use num_traits::AsPrimitive;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bytecode::PathId;
use super::cell;
use super::enum_def::{SEEK_FROM, XML_NODE};
use super::int_methods::{from_bytes, from_bytes_order};
use super::jwt_bridge::jwt_assoc;
use super::native::Native;
use super::numeric::IntWidth;
use super::std_bridge::{
    arg_int, arg_str, as_i64, bytes_to_string, make_duration, make_path, open_file, path_like,
};
use super::value::{CellKind, Value};

/// Associated functions like `String::from`, `File::open`, `Regex::new`.
pub(super) fn assoc_fn(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    if let Some(v) = jwt_assoc(id, args)? {
        return Ok(Some(v));
    }
    // The groups answer disjoint ids, so the first helper that recognizes
    // the id answers.
    if let Some(v) = conversion_assoc(id, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = int_assoc(id, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = container_assoc(id, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = fs_process_assoc(id, args)? {
        return Ok(Some(v));
    }
    misc_assoc(id, args)
}

/// String, char, and numeric constructors and conversions.
fn conversion_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::StringNew | PathId::StringWithCapacity => Value::str(""),
        PathId::StringFrom => Value::str(args.first().map(Value::display).unwrap_or_default()),
        PathId::StringFromUtf8Lossy => Value::str(bytes_to_string(args.first())),
        // `char::from` only converts a u8 in real Rust, so the byte range is
        // enforced even though every integer is an i64 here.
        PathId::CharFrom => match args.first() {
            Some(Value::Char(c)) => Value::Char(*c),
            Some(Value::Int(n)) => match u8::try_from(*n) {
                Ok(b) => Value::Char(char::from(b)),
                Err(_) => bail!("`char::from` needs a u8"),
            },
            _ => bail!("`char::from` needs a u8"),
        },
        PathId::CharFromU32 => match args.first().and_then(as_i64) {
            Some(n) => match u32::try_from(n).ok().and_then(char::from_u32) {
                Some(c) => Value::some(Value::Char(c)),
                None => Value::none(),
            },
            _ => Value::none(),
        },
        PathId::CharFromDigit => {
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
        PathId::StringFromUtf8 => Value::ok(Value::str(bytes_to_string(args.first()))),
        _ => return int_assoc(id, args),
    }))
}

/// The integer constructors and conversions, `from_str_radix`, `from`,
/// `try_from`, and the byte order readers, plus the float `from`.
fn int_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    let ty = id.namespace();
    Ok(Some(match id {
        // Every 64-bit-and-under type parses the same way here, values are
        // untyped ints. The 128-bit types parse in their real width below.
        PathId::I8FromStrRadix
        | PathId::I16FromStrRadix
        | PathId::I32FromStrRadix
        | PathId::I64FromStrRadix
        | PathId::IsizeFromStrRadix
        | PathId::U8FromStrRadix
        | PathId::U16FromStrRadix
        | PathId::U32FromStrRadix
        | PathId::U64FromStrRadix
        | PathId::UsizeFromStrRadix => {
            let text = args.first().map(Value::display).unwrap_or_default();
            let radix = radix_arg(args);
            match i64::from_str_radix(text.trim(), radix) {
                Ok(n) => Value::ok(Value::Int(n)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        PathId::I128FromStrRadix => {
            let text = args.first().map(Value::display).unwrap_or_default();
            match i128::from_str_radix(text.trim(), radix_arg(args)) {
                Ok(n) => Value::ok(Value::Big(n, IntWidth::I128)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        PathId::U128FromStrRadix => {
            let text = args.first().map(Value::display).unwrap_or_default();
            match u128::from_str_radix(text.trim(), radix_arg(args)) {
                Ok(n) => Value::ok(Value::Big(n.cast_signed(), IntWidth::U128)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // Numeric `T::from(x)`. Every integer is an i64 here, so a widening
        // conversion just carries the value. `from` on a bool gives 0 or 1,
        // the same as `usize::from(cond)` and the like.
        PathId::U128From | PathId::I128From => {
            let width = IntWidth::parse(ty).expect("128-bit width parses");
            Value::int_of_width(i128::from(int_from_arg(ty, args.first())?), width)
        }
        PathId::I8From
        | PathId::I16From
        | PathId::I32From
        | PathId::I64From
        | PathId::IsizeFrom
        | PathId::U8From
        | PathId::U16From
        | PathId::U32From
        | PathId::U64From
        | PathId::UsizeFrom => Value::Int(int_from_arg(ty, args.first())?),
        PathId::F32From | PathId::F64From => match args.first() {
            Some(Value::Float(f)) => Value::Float(*f),
            Some(Value::Int(n)) => Value::Float(AsPrimitive::<f64>::as_(*n)),
            Some(Value::Bool(b)) => Value::Float(if *b { 1.0 } else { 0.0 }),
            _ => bail!("`{ty}::from` needs a number"),
        },
        // Fallible `T::try_from(x)`. The value fits when it lands inside the
        // target range, so a narrowing conversion reports overflow with the
        // same message as the real `TryFromIntError`.
        PathId::I8TryFrom
        | PathId::I16TryFrom
        | PathId::I32TryFrom
        | PathId::I64TryFrom
        | PathId::I128TryFrom
        | PathId::IsizeTryFrom
        | PathId::U8TryFrom
        | PathId::U16TryFrom
        | PathId::U32TryFrom
        | PathId::U64TryFrom
        | PathId::U128TryFrom
        | PathId::UsizeTryFrom => {
            let n = int_from_arg(ty, args.first())?;
            if int_fits(ty, n) {
                Value::ok(Value::Int(n))
            } else {
                Value::err(Value::str(
                    "out of range integral type conversion attempted",
                ))
            }
        }
        _ => return int_bytes_assoc(id, args),
    }))
}

/// Containers, wrappers, paths, and regex.
fn container_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // The shape carries every field a later builder call can set, since a
        // shape cannot grow after the instance exists.
        PathId::CommandNew => command_new(args.first().cloned().unwrap_or_else(|| Value::str(""))),
        PathId::VecNew | PathId::VecWithCapacity => Value::vec(vec![]),
        PathId::VecFrom => match args.first() {
            Some(Value::Vec(v)) => Value::vec(v.lock().clone()),
            Some(other) => Value::vec(vec![other.clone()]),
            None => Value::vec(vec![]),
        },
        PathId::HashMapNew | PathId::BTreeMapNew | PathId::HashMapWithCapacity => Value::map(),
        PathId::HashSetNew | PathId::BTreeSetNew | PathId::HashSetWithCapacity => Value::set(),
        // `Rc::clone(&x)` is `x.clone()` spelled as the docs recommend,
        // and a cell's clone shares its slot, so handing the value through
        // is exactly right for both.
        PathId::BoxNew | PathId::RcClone | PathId::ArcClone => {
            args.first().cloned().unwrap_or(Value::Unit)
        }
        // Real shared cells: cloning shares the slot and writes through one
        // handle show through every handle. `Box` above stays transparent,
        // ownership is what the value model already gives every value.
        PathId::RcNew => {
            cell::make_cell(CellKind::Rc, args.first().cloned().unwrap_or(Value::Unit))
        }
        PathId::ArcNew => {
            cell::make_cell(CellKind::Arc, args.first().cloned().unwrap_or(Value::Unit))
        }
        PathId::RefCellNew => cell::make_cell(
            CellKind::RefCell,
            args.first().cloned().unwrap_or(Value::Unit),
        ),
        PathId::CellNew => {
            cell::make_cell(CellKind::Cell, args.first().cloned().unwrap_or(Value::Unit))
        }
        PathId::MutexNew => cell::make_cell(
            CellKind::Mutex,
            args.first().cloned().unwrap_or(Value::Unit),
        ),
        PathId::RcStrongCount | PathId::ArcStrongCount => {
            let Some(Value::Cell(_, slot)) = args.first() else {
                bail!("strong_count needs an Rc or Arc argument");
            };
            // Two in-flight copies exist during this call, the taken arg
            // window value and the bridge's imaging clone of it, which real
            // Rust's borrowed `&Rc` does not have. A borrow passed through
            // further function hops is modeled as a clone per hop, so a
            // count read deep in a call chain can still run high.
            super::shared::usize_value(Arc::strong_count(slot) - 2)
        }
        // Our file and pipe readers are already buffered, so wrapping is a
        // pass-through; a raw socket is turned into a buffered reader.
        PathId::BufReaderNew
        | PathId::BufReaderWithCapacity
        | PathId::BufWriterNew
        | PathId::BufWriterWithCapacity => match args.last() {
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
        PathId::PathBufNew => make_path(""),
        PathId::PathBufFrom | PathId::PathFrom | PathId::PathNew => {
            make_path(args.first().map(path_like).unwrap_or_default())
        }
        PathId::RegexNew => {
            let pat = args.first().map(Value::display).unwrap_or_default();
            match regex::Regex::new(&pat) {
                Ok(compiled) => Value::ok(super::regex_bridge::make_regex(compiled, &pat)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => return Ok(None),
    }))
}

/// `T::from_le_bytes` and its be and ne siblings for every integer width.
fn int_bytes_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // `T::from_le_bytes` and its be and ne siblings. The result carries
        // the named width, so a `u32` read stays a u32 and an `i32` read of
        // the same four bytes is negative where the top bit is set.
        PathId::I8FromLeBytes
        | PathId::I8FromBeBytes
        | PathId::I8FromNeBytes
        | PathId::I16FromLeBytes
        | PathId::I16FromBeBytes
        | PathId::I16FromNeBytes
        | PathId::I32FromLeBytes
        | PathId::I32FromBeBytes
        | PathId::I32FromNeBytes
        | PathId::I64FromLeBytes
        | PathId::I64FromBeBytes
        | PathId::I64FromNeBytes
        | PathId::I128FromLeBytes
        | PathId::I128FromBeBytes
        | PathId::I128FromNeBytes
        | PathId::IsizeFromLeBytes
        | PathId::IsizeFromBeBytes
        | PathId::IsizeFromNeBytes
        | PathId::U8FromLeBytes
        | PathId::U8FromBeBytes
        | PathId::U8FromNeBytes
        | PathId::U16FromLeBytes
        | PathId::U16FromBeBytes
        | PathId::U16FromNeBytes
        | PathId::U32FromLeBytes
        | PathId::U32FromBeBytes
        | PathId::U32FromNeBytes
        | PathId::U64FromLeBytes
        | PathId::U64FromBeBytes
        | PathId::U64FromNeBytes
        | PathId::U128FromLeBytes
        | PathId::U128FromBeBytes
        | PathId::U128FromNeBytes
        | PathId::UsizeFromLeBytes
        | PathId::UsizeFromBeBytes
        | PathId::UsizeFromNeBytes => int_from_bytes(id, args)?,
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
fn fs_process_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::PermissionsFromMode => {
            let mode = args.first().and_then(as_i64).unwrap_or(0o644);
            Value::struct_of("Permissions", vec![("mode".into(), Value::Int(mode))])
        }
        // -- files -----------------------------------------------------
        PathId::FileOpen => open_file(&arg_str(args, 0), std::fs::OpenOptions::new().read(true)),
        PathId::FileCreate => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        ),
        PathId::FileCreateNew => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create_new(true),
        ),
        PathId::OpenOptionsNew => Value::struct_of(
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
        PathId::StdioPiped | PathId::StdioInherit | PathId::StdioNull => {
            Value::struct_of("Stdio", [("kind".into(), Value::str(id.name()))])
        }
        // `Stdio::from(file)` sends a child's stream straight to an open file.
        // The marker carries the file, and the handle is cloned when the
        // command is built so the script keeps its own copy.
        PathId::StdioFrom => {
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
fn misc_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // -- time ------------------------------------------------------
        PathId::InstantNow => Native::Instant(std::time::Instant::now()).wrap(),
        PathId::SystemTimeNow => Native::SystemTime(std::time::SystemTime::now()).wrap(),
        PathId::DurationFromSecs => make_duration(std::time::Duration::from_secs(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        PathId::DurationFromMillis => make_duration(std::time::Duration::from_millis(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        PathId::DurationFromMicros => make_duration(std::time::Duration::from_micros(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        PathId::DurationFromNanos => make_duration(std::time::Duration::from_nanos(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
        )),
        PathId::DurationNew => make_duration(std::time::Duration::new(
            u64::try_from(arg_int(args, 0)).unwrap_or_default(),
            u32::try_from(arg_int(args, 1)).unwrap_or_default(),
        )),
        // -- net -------------------------------------------------------
        PathId::TcpListenerBind => match std::net::TcpListener::bind(arg_str(args, 0)) {
            Ok(l) => Value::ok(Native::Listener(l).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        PathId::TcpStreamConnect => match std::net::TcpStream::connect(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Stream(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        PathId::UdpSocketBind => match std::net::UdpSocket::bind(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Udp(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        PathId::DocumentLoad => super::pdf_bridge::load(&arg_str(args, 0)),
        PathId::ElementParse => super::xmltree_bridge::parse(args),
        PathId::ElementNew => super::xmltree_bridge::new_element(&arg_str(args, 0)),
        // The real xmltree node enum, constructed like `SeekFrom` below since
        // no user declaration exists for it.
        PathId::XMLNodeElement
        | PathId::XMLNodeText
        | PathId::XMLNodeComment
        | PathId::XMLNodeCData
        | PathId::XMLNodeProcessingInstruction => {
            Value::enum_named(&XML_NODE, id.name(), args.to_vec())
                .expect("the matched xmltree node variants are all listed")
        }
        PathId::SeekFromStart | PathId::SeekFromEnd | PathId::SeekFromCurrent => Value::enum_named(
            &SEEK_FROM,
            id.name(),
            vec![args.first().cloned().unwrap_or(Value::Int(0))],
        )
        .expect("the matched SeekFrom variants are all listed"),
        _ => return Ok(None),
    }))
}

/// Pull an integer out of a `from`/`try_from` argument. Ints carry through,
/// a bool becomes 0 or 1, and a char becomes its scalar value.
/// The `u32` radix argument of `from_str_radix`, 10 when unreadable.
fn radix_arg(args: &[Value]) -> u32 {
    args.get(1)
        .and_then(as_i64)
        .and_then(|r| u32::try_from(r).ok())
        .unwrap_or(10)
}

fn int_from_arg(ty: &str, v: Option<&Value>) -> Result<i64> {
    match v {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Bool(b)) => Ok(i64::from(*b)),
        Some(Value::Char(c)) => Ok(*c as i64),
        // A width-tagged or 128-bit value converts when it fits an i64. The
        // callers check the target's own range on top of this.
        Some(tagged @ (Value::IntW(..) | Value::Big(..))) => match tagged.int_parts() {
            Some((n, _)) => i64::try_from(n).map_err(|_| anyhow!("`{ty}` conversion out of range")),
            None => bail!("`{ty}` conversion out of range"),
        },
        _ => bail!("`{ty}` conversion needs an integer"),
    }
}

/// `T::from_le_bytes([..])` and its be and ne siblings, over the same shared
/// core the `to_*_bytes` methods use.
fn int_from_bytes(id: PathId, args: &[Value]) -> Result<Value> {
    let (Some(width), Some(order)) = (IntWidth::parse(id.namespace()), from_bytes_order(id.name()))
    else {
        bail!("`{id}` is not a byte conversion");
    };
    let bytes = byte_array(id, args.first())?;
    Ok(Value::int_of_width(
        from_bytes(width, order, &bytes)?,
        width,
    ))
}

/// The `[u8; N]` argument of a byte conversion. An array literal is a vec at
/// runtime, so the shape real Rust guarantees in its type is read back here.
fn byte_array(id: PathId, arg: Option<&Value>) -> Result<Vec<i128>> {
    let Some(Value::Vec(items)) = arg else {
        bail!("`{id}` needs a byte array");
    };
    let items = items.lock();
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter() {
        let Some((value, _)) = item.int_parts() else {
            bail!("`{id}` needs a byte array");
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
