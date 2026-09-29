#!/usr/bin/env rust

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::process::Command;

// Binds the way a small server picks a free port, and reads real io error kinds.
fn main() -> std::io::Result<()> {
    let first = TcpListener::bind(("127.0.0.1", 0))?;
    let port = first.local_addr()?.port();
    println!("tuple bind: {}", port > 0);

    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => println!("bound twice"),
        Err(e) if e.kind() == ErrorKind::AddrInUse => println!("in use"),
        Err(e) => println!("other {e}"),
    }
    match TcpListener::bind(format!("127.0.0.1:{port}")) {
        Ok(_) => println!("bound twice"),
        Err(e) => println!("string form: {:?}", e.kind()),
    }

    let addr = first.local_addr()?;
    let client = TcpStream::connect(addr)?;
    println!("connect by addr: {}", client.peer_addr()? == addr);
    let again = TcpStream::connect(("127.0.0.1", port))?;
    println!("connect by tuple: {}", again.peer_addr()? == addr);

    match TcpListener::bind("not an address") {
        Ok(_) => println!("parsed"),
        Err(e) => println!("bad address: {:?} {e}", e.kind()),
    }

    let a = UdpSocket::bind(("127.0.0.1", 0))?;
    let b = UdpSocket::bind("127.0.0.1:0")?;
    a.send_to(b"hi", b.local_addr()?)?;
    let mut buf = [0u8; 8];
    let (n, from) = b.recv_from(&mut buf)?;
    println!(
        "udp: {} from a: {}",
        String::from_utf8_lossy(&buf[..n]),
        from == a.local_addr()?
    );
    b.connect(("127.0.0.1", a.local_addr()?.port()))?;
    b.send(b"back")?;
    let n = a.recv(&mut buf)?;
    println!("udp back: {}", String::from_utf8_lossy(&buf[..n]));

    match Command::new("rustscript-no-such-command").output() {
        Ok(_) => println!("ran"),
        Err(e) => println!("spawn: {:?} {}", e.kind(), e.kind() == ErrorKind::NotFound),
    }
    Ok(())
}
