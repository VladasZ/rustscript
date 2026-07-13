use std::io::{BufRead, BufReader, Write, stdout};
use std::net::{TcpListener, TcpStream};
use std::thread;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    println!("http://{address}");
    stdout().flush()?;

    for stream in listener.incoming() {
        let stream = stream?;
        thread::spawn(move || {
            if let Err(error) = handle(stream) {
                eprintln!("HTTP connection failed: {error}");
            }
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut request = String::new();
        if reader.read_line(&mut request)? == 0 {
            return Ok(());
        }
        if request == "\r\n" {
            continue;
        }

        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                return Ok(());
            }
            if header == "\r\n" {
                break;
            }
        }

        let path = request
            .split_whitespace()
            .nth(1)
            .context("malformed request")?;
        let id: u64 = path.trim_start_matches("/item/").parse()?;
        let value = (id * 17 + 11) % 1_000;
        let body = format!(r#"{{"id":{id},"value":{value}}}"#);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
            body.len()
        )?;
        stream.flush()?;
    }
}
