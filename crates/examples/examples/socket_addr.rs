#!/usr/bin/env rust

use std::net::{TcpListener, TcpStream, UdpSocket};

// The port is picked by the OS, so only its relations are printed.
fn main() -> std::io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let addr = socket.local_addr()?;
    println!("ip: {}", addr.ip());
    println!("ip debug: {:?}", addr.ip());
    println!("port set: {}", addr.port() != 0);
    println!("v4: {} v6: {}", addr.is_ipv4(), addr.is_ipv6());

    let ip = addr.ip();
    println!("loopback: {}", ip.is_loopback());
    println!("unspecified: {}", ip.is_unspecified());
    println!("multicast: {}", ip.is_multicast());
    println!("ip v4: {} v6: {}", ip.is_ipv4(), ip.is_ipv6());

    let text = addr.to_string();
    println!(
        "text ends with port: {}",
        text.ends_with(&addr.port().to_string())
    );
    println!(
        "display matches: {}",
        format!("{addr}") == format!("{}:{}", addr.ip(), addr.port())
    );
    println!("debug matches: {}", format!("{addr:?}") == text);

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let server_addr = listener.local_addr()?;
    let client = TcpStream::connect(server_addr)?;
    let (stream, peer) = listener.accept()?;
    println!("peer is client: {}", peer == client.local_addr()?);
    println!("client sees server: {}", client.peer_addr()? == server_addr);
    println!(
        "stream local is server: {}",
        stream.local_addr()? == server_addr
    );
    println!("same ip: {}", peer.ip() == server_addr.ip());
    Ok(())
}
