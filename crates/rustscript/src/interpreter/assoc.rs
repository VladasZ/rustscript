//! Associated functions like `String::from` and `File::open`.

use num_traits::AsPrimitive;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bridge::arg;
use super::bytecode::PathId;
use super::cell;
use super::enum_def::{SEEK_FROM, XML_NODE};
use super::int_methods::{ByteOrder, from_bytes, from_bytes_order};
use super::jwt_bridge::jwt_assoc;
use super::native::Native;
use super::numeric::IntWidth;
use super::std_bridge::{
    arg_int, arg_str, as_i64, bytes_to_string, make_duration, make_path, open_file, path_like,
};
use super::value::{CellKind, Value};

pub(super) fn assoc_fn(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    if let Some(v) = jwt_assoc(id, args)? {
        return Ok(Some(v));
    }
    // the groups handle disjoint ids
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

fn conversion_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::StringNew | PathId::StringWithCapacity => Value::str(""),
        PathId::StringFrom => Value::str(args.first().map(Value::display).unwrap_or_default()),
        PathId::StringFromUtf8Lossy => Value::str(bytes_to_string(args.first())),
        // `char::from` only converts a u8
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

fn int_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // the 128 bit types parse in their real width below
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
        // a widening `T::from(x)` keeps the value, `from` on a bool gives 0 or 1
        PathId::U128From | PathId::I128From => {
            let width = if id == PathId::U128From {
                IntWidth::U128
            } else {
                IntWidth::I128
            };
            Value::int_of_width(i128::from(int_from_arg(id, args.first())?), width)
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
        | PathId::UsizeFrom => Value::Int(int_from_arg(id, args.first())?),
        PathId::F32From | PathId::F64From => match args.first() {
            Some(Value::Float(f)) => Value::Float(*f),
            Some(Value::Int(n)) => Value::Float(AsPrimitive::<f64>::as_(*n)),
            Some(Value::Bool(b)) => Value::Float(if *b { 1.0 } else { 0.0 }),
            _ => bail!("`{}::from` needs a number", id.namespace()),
        },
        // `T::try_from(x)` reports overflow with the real `TryFromIntError` message
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
            let n = int_from_arg(id, args.first())?;
            if int_fits(id, n) {
                Value::ok(Value::Int(n))
            } else {
                Value::err(Value::str(
                    "out of range integral type conversion attempted",
                ))
            }
        }
        _ => {
            return float_bytes_assoc(id, args).and_then(|v| match v {
                Some(value) => Ok(Some(value)),
                None => int_bytes_assoc(id, args),
            });
        }
    }))
}

fn container_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // a shape can't grow after the instance exists, so it has every field a builder call can set
        PathId::CommandNew => command_new(args.first().cloned().unwrap_or_else(|| Value::str(""))),
        PathId::VecNew | PathId::VecWithCapacity => Value::vec(vec![]),
        PathId::VecFrom => match args.first() {
            Some(Value::Vec(v)) => Value::vec(v.lock().clone()),
            Some(other) => Value::vec(vec![other.clone()]),
            None => Value::vec(vec![]),
        },
        PathId::HashMapNew | PathId::BTreeMapNew | PathId::HashMapWithCapacity => Value::map(),
        PathId::HashSetNew | PathId::BTreeSetNew | PathId::HashSetWithCapacity => Value::set(),
        // `Rc::clone(&x)` is `x.clone()`, and a cell clone shares its slot
        PathId::BoxNew | PathId::RcClone | PathId::ArcClone => arg(args, 0)?,
        // real shared cells, `Box` above stays transparent
        PathId::RcNew => cell::make_cell(CellKind::Rc, arg(args, 0)?),
        PathId::ArcNew => cell::make_cell(CellKind::Arc, arg(args, 0)?),
        PathId::RefCellNew => cell::make_cell(CellKind::RefCell, arg(args, 0)?),
        PathId::CellNew => cell::make_cell(CellKind::Cell, arg(args, 0)?),
        PathId::MutexNew => cell::make_cell(CellKind::Mutex, arg(args, 0)?),
        PathId::RcStrongCount | PathId::ArcStrongCount => {
            let Some(Value::Cell(_, slot)) = args.first() else {
                bail!("strong_count needs an Rc or Arc argument");
            };
            // 2 in flight copies exist during this call that a real `&Rc` doesn't have, a borrow
            // through more hops is a clone per hop
            super::shared::usize_value(Arc::strong_count(slot) - 2)
        }
        // file and pipe readers are already buffered, a raw socket gets one
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
            Some(other) => other.clone(),
            None => bail!("missing argument 1"),
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

/// `f32::from_le_bytes` and friends. The bit pattern is read at the named width, so an
/// f32 read of four bytes is not the f64 read of the same bytes widened.
fn float_bytes_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    let narrow = match id {
        PathId::F32FromLeBytes | PathId::F32FromBeBytes | PathId::F32FromNeBytes => true,
        PathId::F64FromLeBytes | PathId::F64FromBeBytes | PathId::F64FromNeBytes => false,
        _ => return Ok(None),
    };
    let Some(order) = from_bytes_order(id.name()) else {
        bail!("`{id}` is not a byte conversion");
    };
    let bytes = byte_array(id, args.first())?;
    let width = if narrow { 4 } else { 8 };
    if bytes.len() != width {
        bail!("`{id}` needs {width} bytes, got {}", bytes.len());
    }
    let mut raw = Vec::with_capacity(width);
    for byte in &bytes {
        raw.push(u8::try_from(*byte)?);
    }
    if matches!(order, ByteOrder::Be)
        || (matches!(order, ByteOrder::Ne) && cfg!(target_endian = "big"))
    {
        raw.reverse();
    }
    Ok(Some(if narrow {
        Value::F32(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    } else {
        Value::Float(f64::from_le_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]))
    }))
}

fn int_bytes_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // the result has the named width, so an `i32` read of the same bytes is negative when the
        // top bit is set
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

fn fs_process_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        PathId::PermissionsFromMode => {
            let mode = args.first().and_then(as_i64).unwrap_or(0o644);
            Value::struct_of("Permissions", vec![("mode".into(), Value::Int(mode))])
        }
        // files
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
        // `Stdio::from(file)`, the handle is cloned when the command is built so the script keeps
        // its own copy
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

fn misc_assoc(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // time
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
        // net
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
        // the real xmltree node enum, constructed like `SeekFrom` below
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

/// The `u32` radix argument of `from_str_radix`, 10 when unreadable.
fn radix_arg(args: &[Value]) -> u32 {
    args.get(1)
        .and_then(as_i64)
        .and_then(|r| u32::try_from(r).ok())
        .unwrap_or(10)
}

/// The path name is only spelled out in the error, `namespace` splits strings and this runs on
/// every `i64::try_from`.
fn int_from_arg(id: PathId, v: Option<&Value>) -> Result<i64> {
    match v {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Bool(b)) => Ok(i64::from(*b)),
        Some(Value::Char(c)) => Ok(*c as i64),
        // the callers check the target's own range on top of this
        Some(tagged @ (Value::IntW(..) | Value::Big(..))) => match tagged.int_parts() {
            Some((n, _)) => i64::try_from(n)
                .map_err(|_| anyhow!("`{}` conversion out of range", id.namespace())),
            None => bail!("`{}` conversion out of range", id.namespace()),
        },
        _ => bail!("`{}` conversion needs an integer", id.namespace()),
    }
}

/// Same shared core as the `to_*_bytes` methods.
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

/// An array literal is a vec at runtime, so the `[u8; N]` shape is read back here.
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

fn int_fits(id: PathId, n: i64) -> bool {
    match id {
        PathId::I8TryFrom => i8::try_from(n).is_ok(),
        PathId::I16TryFrom => i16::try_from(n).is_ok(),
        PathId::I32TryFrom => i32::try_from(n).is_ok(),
        PathId::U8TryFrom => u8::try_from(n).is_ok(),
        PathId::U16TryFrom => u16::try_from(n).is_ok(),
        PathId::U32TryFrom => u32::try_from(n).is_ok(),
        PathId::U64TryFrom | PathId::U128TryFrom | PathId::UsizeTryFrom => n >= 0,
        _ => true,
    }
}
