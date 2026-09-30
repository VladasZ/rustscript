#!/usr/bin/env rust

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use reqwest::Client;

// Answers a GET with binary bytes and echoes a POST back with its content type.
fn answer(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut length = 0;
    let mut content_type = String::new();
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(value) = lower.strip_prefix("content-length: ") {
            length = value.parse().unwrap_or(0);
        }
        if let Some(value) = lower.strip_prefix("content-type: ") {
            content_type = value.to_string();
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;

    let reply: Vec<u8> = if request_line.starts_with("GET") {
        vec![0x25, 0x50, 0x44, 0x46, 0x00, 0xff, 0x10]
    } else {
        format!("{content_type} {}", String::from_utf8_lossy(&body)).into_bytes()
    };
    let mut stream = reader.into_inner();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&reply)?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}/", listener.local_addr()?);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            if let Ok((stream, _)) = listener.accept() {
                answer(stream).unwrap();
            }
        }
    });

    let client = Client::new();
    let resp = client.get(&url).send().await?;
    let bytes = resp.bytes().await?;
    println!("len: {}", bytes.len());
    println!("pdf: {}", bytes.starts_with(b"%PDF"));
    println!("tail: {:?}", &bytes[4..]);
    let path = std::env::temp_dir().join("rustscript_http_bytes_form.bin");
    std::fs::write(&path, &bytes)?;
    println!("written: {:?}", std::fs::read(&path)?);
    std::fs::remove_file(&path)?;

    let resp = client
        .post(&url)
        .form(&[("name", "a b"), ("expr", "x&y=z")])
        .send()
        .await?;
    println!("status: {}", resp.status());
    println!("echo: {}", resp.text().await?);

    server.await?;
    Ok(())
}
