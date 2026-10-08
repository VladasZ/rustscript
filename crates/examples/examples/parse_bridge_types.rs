#!/usr/bin/env rust

//! `parse` into a type of a bridged crate runs the `FromStr` of that crate, like
//! `T::from_str` does. It used to give an `Err` for every text, the builtin `parse` knows only
//! the std scalars.

use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, NaiveDateTime, Timelike, Utc};
use jsonwebtoken::Algorithm;
use regex::Regex;
use reqwest::StatusCode;
use reqwest::header::HeaderValue;

fn toml_types() {
    let text = "[a]\nb = 1\n";
    match text.parse::<toml::Table>() {
        Ok(table) => println!("table: ok, {} keys", table.len()),
        Err(err) => println!("table: error: {err}"),
    }
    match toml::from_str::<toml::Table>(text) {
        Ok(table) => println!("from_str: ok, {} keys", table.len()),
        Err(err) => println!("from_str: error: {err}"),
    }
    match "a = [1, 2".parse::<toml::Table>() {
        Ok(table) => println!("broken table: ok, {} keys", table.len()),
        Err(err) => println!("broken table: error: {err}"),
    }
    let table: toml::Table = "name = \"far\"\nport = 7420\n".parse().unwrap();
    println!(
        "annotated table: {} keys, port {}",
        table.len(),
        table.contains_key("port")
    );
    toml_values(&table);
    match toml::Table::from_str("x = 1\ny = 2\n") {
        Ok(table) => println!("Table::from_str: {} keys", table.len()),
        Err(err) => println!("Table::from_str: error: {err}"),
    }

    // a `toml::Value` parses 1 value, a whole document is an error
    match "42".parse::<toml::Value>() {
        Ok(value) => println!("value: {value}"),
        Err(err) => println!("value: error: {err}"),
    }
    println!(
        "document as value: {}",
        text.parse::<toml::Value>().is_err()
    );
}

/// A value read out of a table is a `toml::Value`. It prints the toml way, a string in quotes,
/// and `as_integer` reads it.
fn toml_values(table: &toml::Table) {
    println!("read by index: {} {}", table["name"], table["port"]);
    println!(
        "as_str: {:?} {:?}",
        table["name"].as_str(),
        table["port"].as_str()
    );
    println!("as_str is some: {}", table["name"].as_str() == Some("far"));
    let quoted = table["name"].to_string();
    println!("to_string: {quoted} of {} bytes", quoted.len());
    println!("as_integer: {:?}", table["port"].as_integer());
    println!("string as_integer: {:?}", table["name"].as_integer());
    let port = table.get("port").and_then(toml::Value::as_integer);
    println!("get: {port:?}");
    println!("missing: {}", table.get("host").is_none());
    for (key, value) in table {
        println!("entry: {key} = {value}");
    }
    println!("debug: {table:?}");
    print!("document:\n{table}");
    let nested: toml::Value = "{ list = [1, 2.5, \"x\"], on = true }".parse().unwrap();
    println!("nested: {nested}");
    println!("nested list: {}", nested["list"]);
    println!("float as_integer: {:?}", nested["list"][1].as_integer());
    println!("nested debug: {:?}", nested["list"]);
}

fn toml_value_by_import() {
    use toml::Value;
    match "true".parse::<Value>() {
        Ok(value) => println!("imported value: {value}"),
        Err(err) => println!("imported value: error: {err}"),
    }
}

fn json_value() {
    match r#"{"a": [1, 2]}"#.parse::<serde_json::Value>() {
        Ok(value) => println!("json: {} {:?}", value["a"][1], value["a"].as_str()),
        Err(err) => println!("json: error: {err}"),
    }
    let name = r#""far""#.parse::<serde_json::Value>().unwrap();
    println!(
        "json as_str: {:?} {}",
        name.as_str(),
        name.as_str().unwrap_or("none")
    );
    match "{".parse::<serde_json::Value>() {
        Ok(value) => println!("broken json: {value}"),
        Err(err) => println!("broken json: error: {err}"),
    }
}

fn chrono_types() {
    match "2026-10-08".parse::<NaiveDate>() {
        Ok(date) => println!("date: {} {} {}", date.year(), date.month(), date.day()),
        Err(err) => println!("date: error: {err}"),
    }
    match "2026-13-08".parse::<NaiveDate>() {
        Ok(date) => println!("bad date: {}", date.year()),
        Err(err) => println!("bad date: error: {err}"),
    }
    let date: NaiveDate = "2024-02-29".parse().unwrap();
    println!("annotated date: {}", date.format("%d.%m.%Y"));
    match NaiveDate::from_str("2025-01-02") {
        Ok(date) => println!("NaiveDate::from_str: day {} of the year", date.ordinal()),
        Err(err) => println!("NaiveDate::from_str: error: {err}"),
    }
    match "2026-10-08T10:11:12".parse::<NaiveDateTime>() {
        Ok(at) => println!("naive: {}", at.and_utc().timestamp()),
        Err(err) => println!("naive: error: {err}"),
    }

    // the same instant, the offset stays only in a `DateTime<FixedOffset>`
    let text = "2026-10-08T10:11:12+02:00";
    match text.parse::<DateTime<FixedOffset>>() {
        Ok(at) => println!(
            "fixed: {} hour {} day {}",
            at.to_rfc3339(),
            at.hour(),
            at.ordinal()
        ),
        Err(err) => println!("fixed: error: {err}"),
    }
    match text.parse::<DateTime<Utc>>() {
        Ok(at) => println!("utc: {} hour {}", at.to_rfc3339(), at.hour()),
        Err(err) => println!("utc: error: {err}"),
    }
    match text.parse::<DateTime<Local>>() {
        Ok(at) => println!("local: {}", at.timestamp()),
        Err(err) => println!("local: error: {err}"),
    }
    let at: DateTime<Utc> = "2026-01-02T03:04:05Z".parse().unwrap();
    println!("annotated utc: {}", at.timestamp());
    match DateTime::<Utc>::from_str(text) {
        Ok(at) => println!("DateTime::<Utc>::from_str: {}", at.to_rfc3339()),
        Err(err) => println!("DateTime::<Utc>::from_str: error: {err}"),
    }
    match "yesterday".parse::<DateTime<Utc>>() {
        Ok(at) => println!("bad utc: {}", at.timestamp()),
        Err(err) => println!("bad utc: error: {err}"),
    }
}

fn first_address(text: &str) -> Result<IpAddr, AddrParseError> {
    let first = text.split(',').next().unwrap_or_default();
    first.trim().parse()
}

fn net_types() {
    match "127.0.0.1".parse::<IpAddr>() {
        Ok(ip) => println!("ip: {ip} v4 {} loopback {}", ip.is_ipv4(), ip.is_loopback()),
        Err(err) => println!("ip: error: {err}"),
    }
    match "::1".parse::<IpAddr>() {
        Ok(ip) => println!("ip6: {ip} v6 {}", ip.is_ipv6()),
        Err(err) => println!("ip6: error: {err}"),
    }
    match "300.0.0.1".parse::<IpAddr>() {
        Ok(ip) => println!("bad ip: {ip}"),
        Err(err) => println!("bad ip: error: {err}"),
    }
    match "10.0.0.7:8080".parse::<SocketAddr>() {
        Ok(addr) => println!("socket: {addr} ip {} port {}", addr.ip(), addr.port()),
        Err(err) => println!("socket: error: {err}"),
    }
    match first_address(" 192.168.1.1 , 10.0.0.1") {
        Ok(ip) => println!("inferred ip: {ip}"),
        Err(err) => println!("inferred ip: error: {err}"),
    }
    match IpAddr::from_str("10.1.2.3") {
        Ok(ip) => println!("IpAddr::from_str: {ip}"),
        Err(err) => println!("IpAddr::from_str: error: {err}"),
    }
    let good: Vec<IpAddr> = ["1.1.1.1", "nope", "8.8.8.8"]
        .iter()
        .filter_map(|text| text.parse::<IpAddr>().ok())
        .collect();
    println!("in a closure: {}", good.len());
}

fn other_types() {
    let path = "logs/app.log".parse::<PathBuf>().unwrap();
    println!("path: {} name {:?}", path.display(), path.file_name());

    match r"^\d+-\d+$".parse::<Regex>() {
        Ok(re) => println!("regex: {} {}", re.is_match("12-34"), re.is_match("12")),
        Err(err) => println!("regex: error: {err}"),
    }
    println!("bad regex: {}", "(".parse::<Regex>().is_err());

    match "text/plain".parse::<HeaderValue>() {
        Ok(value) => println!("header: {:?}", value.to_str()),
        Err(err) => println!("header: error: {err}"),
    }
    println!("bad header: {}", "a\nb".parse::<HeaderValue>().is_err());

    match "404".parse::<StatusCode>() {
        Ok(status) => println!("status: {} {}", status.as_u16(), status.is_client_error()),
        Err(err) => println!("status: error: {err}"),
    }
    match "abc".parse::<StatusCode>() {
        Ok(status) => println!("bad status: {}", status.as_u16()),
        Err(err) => println!("bad status: error: {err}"),
    }

    match "HS384".parse::<Algorithm>() {
        Ok(algorithm) => println!("algorithm: {algorithm:?}"),
        Err(err) => println!("algorithm: error: {err}"),
    }
    println!("bad algorithm: {}", "HS1".parse::<Algorithm>().is_err());
}

fn main() {
    toml_types();
    toml_value_by_import();
    json_value();
    chrono_types();
    net_types();
    other_types();
}
