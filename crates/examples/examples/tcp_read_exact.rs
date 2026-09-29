#!/usr/bin/env rust

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};

// Reads a POST body by its Content-Length, the way a tiny HTTP server does.
fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut client = TcpStream::connect(listener.local_addr()?)?;
    client.write_all(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello tail")?;
    client.shutdown(std::net::Shutdown::Write)?;

    let (server, _) = listener.accept()?;
    let mut reader = BufReader::new(server);
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse().unwrap_or(0);
        }
    }

    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    println!("body: {}", String::from_utf8_lossy(&body));

    let mut arr = [0u8; 3];
    reader.read_exact(&mut arr)?;
    println!("next: {arr:?}");

    let mut too_long = vec![0u8; 10];
    match reader.read_exact(&mut too_long) {
        Ok(()) => println!("unexpected full read"),
        Err(e) => println!("eof: {}", e.kind() == ErrorKind::UnexpectedEof),
    }
    Ok(())
}
