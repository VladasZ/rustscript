#!/usr/bin/env rust

// A tiny loopback exchange: bind a listener, connect to it, send a line, and
// read it back on the server side.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let mut client = TcpStream::connect(addr)?;
    client.write_all(b"ping\n")?;
    client.flush()?;

    let (server, _peer) = listener.accept()?;
    let mut reader = BufReader::new(server);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    println!("server received: {}", line.trim());
    Ok(())
}
