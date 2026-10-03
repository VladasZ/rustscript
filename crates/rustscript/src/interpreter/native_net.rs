//! Net handles and the address values they hand out, `TcpListener`, `TcpStream`, `UdpSocket`,
//! `SocketAddr` and `IpAddr`.

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, MethodName};
use super::native::Native;
use super::native_methods::{Handle, buffer_len, fill_buffer, int_len, io_err, value_to_bytes};
use super::numeric::IntWidth;
use super::value::Value;

/// A net address argument through the real `ToSocketAddrs`, so a `"host:port"` string, a
/// `(host, port)` tuple and a `SocketAddr` all resolve like in compiled Rust.
pub(super) fn socket_addrs(arg: Option<&Value>) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::net::ToSocketAddrs;
    let invalid =
        || std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid socket address");
    let value = match arg {
        Some(Value::Ref(reference)) => reference.get(),
        other => other.cloned(),
    };
    match value {
        Some(Value::Native(n)) => match &*n.lock() {
            Native::SocketAddr(addr) => Ok(vec![*addr]),
            _ => Err(invalid()),
        },
        Some(Value::Tuple(items)) => {
            let items = items.lock();
            let [host, port] = items.as_slice() else {
                return Err(invalid());
            };
            let port = port
                .int_parts()
                .and_then(|(p, _)| u16::try_from(p).ok())
                .ok_or_else(invalid)?;
            Ok((host.display().as_str(), port).to_socket_addrs()?.collect())
        }
        Some(other) => Ok(super::std_bridge::path_like(&other)
            .to_socket_addrs()?
            .collect()),
        None => Err(invalid()),
    }
}

pub(super) fn socket_addr_method(handle: &Handle, method: &MethodName) -> Option<Value> {
    let Native::SocketAddr(addr) = *handle.lock() else {
        return None;
    };
    match method.id {
        BuiltinId::Ip => Some(Native::IpAddr(addr.ip()).wrap()),
        BuiltinId::Port => Some(Value::int_of_width(i128::from(addr.port()), IntWidth::U16)),
        BuiltinId::IsIpv4 => Some(Value::Bool(addr.is_ipv4())),
        BuiltinId::IsIpv6 => Some(Value::Bool(addr.is_ipv6())),
        _ => None,
    }
}

pub(super) fn ip_addr_method(handle: &Handle, method: &MethodName) -> Option<Value> {
    let Native::IpAddr(ip) = *handle.lock() else {
        return None;
    };
    match method.id {
        BuiltinId::IsIpv4 => Some(Value::Bool(ip.is_ipv4())),
        BuiltinId::IsIpv6 => Some(Value::Bool(ip.is_ipv6())),
        BuiltinId::IsLoopback => Some(Value::Bool(ip.is_loopback())),
        BuiltinId::IsUnspecified => Some(Value::Bool(ip.is_unspecified())),
        BuiltinId::IsMulticast => Some(Value::Bool(ip.is_multicast())),
        _ => None,
    }
}
pub(super) fn net_native_method(handle: &Handle, method: &MethodName) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Accept => {
            let h = handle.lock();
            if let Native::Listener(l) = &*h {
                return Ok(Some(match l.accept() {
                    Ok((stream, addr)) => Value::ok(Value::tuple(vec![
                        Native::Stream(stream).wrap(),
                        Native::SocketAddr(addr).wrap(),
                    ])),
                    Err(e) => Value::err(super::native::io_error_value(&e)),
                }));
            }
            bail!("accept on non-listener {}", h.type_name());
        }
        BuiltinId::Incoming => {
            bail!("incoming() is not supported; loop with listener.accept() instead");
        }
        BuiltinId::LocalAddr => {
            let h = handle.lock();
            let addr = match &*h {
                Native::Listener(l) => l.local_addr(),
                Native::Stream(s) => s.local_addr(),
                Native::Udp(s) => s.local_addr(),
                _ => bail!("local_addr on {}", h.type_name()),
            };
            return Ok(Some(io_err(addr, |a| Native::SocketAddr(a).wrap())));
        }
        BuiltinId::PeerAddr => {
            let h = handle.lock();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(s.peer_addr(), |a| {
                    Native::SocketAddr(a).wrap()
                })));
            }
            bail!("peer_addr on {}", h.type_name());
        }
        BuiltinId::Shutdown => {
            let h = handle.lock();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(s.shutdown(std::net::Shutdown::Both), |()| {
                    Value::Unit
                })));
            }
            bail!("shutdown on {}", h.type_name());
        }
        BuiltinId::TryClone => {
            let h = handle.lock();
            match &*h {
                Native::Stream(s) => {
                    return Ok(Some(io_err(s.try_clone(), |s| Native::Stream(s).wrap())));
                }
                Native::Udp(s) => {
                    return Ok(Some(io_err(s.try_clone(), |s| Native::Udp(s).wrap())));
                }
                _ => bail!("try_clone on {}", h.type_name()),
            }
        }
        _ => {}
    }
    Ok(None)
}

pub(super) fn udp_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::SetBroadcast => {
            let on = matches!(args.first(), Some(Value::Bool(true)));
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.set_broadcast(on), |()| Value::Unit)));
            }
            bail!("set_broadcast on {}", h.type_name());
        }
        BuiltinId::SendTo => {
            let bytes = value_to_bytes(args.first());
            let addrs = socket_addrs(args.get(1));
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                let sent = addrs.and_then(|a| s.send_to(&bytes, &a[..]));
                return Ok(Some(io_err(sent, |n| Value::Int(int_len(n)))));
            }
            bail!("send_to on {}", h.type_name());
        }
        BuiltinId::Recv | BuiltinId::RecvFrom => {
            let mut buf = vec![0u8; buffer_len(args.first())];
            let h = handle.lock();
            let Native::Udp(s) = &*h else {
                bail!("{} on {}", method.id.name(), h.type_name());
            };
            let got = if method.id == BuiltinId::Recv {
                s.recv(&mut buf).map(|n| (n, None))
            } else {
                s.recv_from(&mut buf).map(|(n, from)| (n, Some(from)))
            };
            drop(h);
            return Ok(Some(io_err(got, |(n, from)| {
                fill_buffer(args.first(), &buf[..n]);
                match from {
                    Some(addr) => Value::tuple(vec![
                        Value::Int(int_len(n)),
                        Native::SocketAddr(addr).wrap(),
                    ]),
                    None => Value::Int(int_len(n)),
                }
            })));
        }
        BuiltinId::Send => {
            let bytes = value_to_bytes(args.first());
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.send(&bytes), |n| Value::Int(int_len(n)))));
            }
            bail!("send on {}", h.type_name());
        }
        BuiltinId::Connect => {
            let addrs = socket_addrs(args.first());
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                let connected = addrs.and_then(|a| s.connect(&a[..]));
                return Ok(Some(io_err(connected, |()| Value::Unit)));
            }
            bail!("connect on {}", h.type_name());
        }
        _ => {}
    }
    Ok(None)
}
