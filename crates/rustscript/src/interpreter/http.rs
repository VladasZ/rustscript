//! The ureq backed http bridge. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::native::Native;

use super::value::{StructData, Value};

use super::json_bridge::*;
use super::std_bridge::*;


// -- ureq http bridge ------------------------------------------------------

/// Build an `HttpRequest` value for `ureq::get`, `ureq::post`, and friends.
/// `ureq::agent()` instead builds a cookie-persisting agent handle.
pub(super) fn make_request(func: &str, args: &[Value]) -> Option<Value> {
    if func == "agent" {
        return Some(Native::Agent(ureq::agent()).wrap());
    }
    let method = http_verb(func)?;
    Some(build_http_request(method, args.first(), None))
}

pub(super) fn http_verb(func: &str) -> Option<&'static str> {
    Some(match func {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        _ => return None,
    })
}

/// Build an `HttpRequest`, optionally bound to an agent so its cookie jar and
/// config carry through the call. Fields a builder call can set later hold
/// Unit placeholders, since a shape cannot grow after the instance exists.
pub(super) fn build_http_request(method: &str, url: Option<&Value>, agent: Option<Value>) -> Value {
    Value::struct_of(
        "HttpRequest",
        [
            ("method".into(), Value::str(method)),
            ("url".into(), Value::str(url.map(|v| v.display()).unwrap_or_default())),
            ("headers".into(), Value::vec(vec![])),
            ("agent".into(), agent.unwrap_or(Value::Unit)),
            ("query".into(), Value::Unit),
            ("timeout".into(), Value::Unit),
        ],
    )
}

pub(super) fn http_method(
    s: &Rc<StructData>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    match &**s.name() {
        "HttpRequest" => request_method(s, method, args),
        "HttpResponse" => Ok(response_method(s, method)),
        "HttpBody" => body_method(s, method),
        "StatusCode" => Ok(status_method(s, method)),
        "HeaderMap" => Ok(header_map_method(s, method, args)),
        "HeaderValue" => Ok(header_value_method(s, method)),
        _ => bail!("unknown http method `{method}`"),
    }
}

pub(super) fn request_method(
    s: &Rc<StructData>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    match method {
        "header" | "set" | "content_type" => {
            let pair = if method == "content_type" {
                vec![Value::str("Content-Type"), args.first().cloned().unwrap_or(Value::Unit)]
            } else {
                vec![
                    args.first().cloned().unwrap_or(Value::Unit),
                    args.get(1).cloned().unwrap_or(Value::Unit),
                ]
            };
            if let Some(Value::Vec(h)) = s.get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            }
            Ok(this())
        }
        "call" => Ok(run_request(s, None)),
        "send" | "send_string" => {
            let body = args.first().map(|v| v.display()).unwrap_or_default();
            Ok(run_request(s, Some(body)))
        }
        "send_json" => {
            let json = value_to_json(args.first().unwrap_or(&Value::Unit))?;
            if let Some(Value::Vec(h)) = s.get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(vec![
                    Value::str("Content-Type"),
                    Value::str("application/json"),
                ]))));
            }
            Ok(run_request(s, Some(serde_json::to_string(&json)?)))
        }
        "query" => {
            let pair = vec![
                args.first().cloned().unwrap_or(Value::Unit),
                args.get(1).cloned().unwrap_or(Value::Unit),
            ];
            let q = match s.get("query") {
                Some(Value::Vec(q)) => q,
                _ => {
                    let q = Rc::new(RefCell::new(vec![]));
                    s.set("query", Value::Vec(q.clone()));
                    q
                }
            };
            q.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            Ok(this())
        }
        // ureq 3 sets timeouts through `.config().timeout_global(Some(d)).build()`.
        // `config` and `build` are pass-throughs; the timeout is stored for the call.
        "config" | "build" => Ok(this()),
        "timeout" | "timeout_global" | "timeout_connect" => {
            // The argument may be a bare Duration or an Option<Duration>.
            let dur = match args.first() {
                Some(Value::Enum { data, .. }) => data.first().cloned(),
                other => other.cloned(),
            };
            if let Some(d) = dur {
                s.set("timeout", d);
            }
            Ok(this())
        }
        _ => bail!("unknown method `{method}` on a request"),
    }
}

/// Percent-encode a query value the simple way, enough for API params.
pub(super) fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub(super) fn run_request(s: &StructData, body: Option<String>) -> Value {
    let verb = s.get("method").map(|v| v.display()).unwrap_or_else(|| "GET".into());
    let mut url = s.get("url").map(|v| v.display()).unwrap_or_default();
    // Append any query parameters onto the URL.
    if let Some(Value::Vec(q)) = s.get("query") {
        let q = q.borrow();
        if !q.is_empty() {
            let sep = if url.contains('?') { '&' } else { '?' };
            url.push(sep);
            let parts: Vec<String> = q
                .iter()
                .filter_map(|item| {
                    if let Value::Tuple(pair) = item {
                        let pair = pair.borrow();
                        Some(format!(
                            "{}={}",
                            encode_query(&pair[0].display()),
                            encode_query(&pair[1].display())
                        ))
                    } else {
                        None
                    }
                })
                .collect();
            url.push_str(&parts.join("&"));
        }
    }
    let timeout = s.get("timeout").and_then(|v| duration_from_value(&v));
    let agent = match s.get("agent") {
        Some(Value::Native(h)) => Some(h),
        _ => None,
    };
    let mut headers = Vec::new();
    if let Some(Value::Vec(h)) = s.get("headers") {
        for item in h.borrow().iter() {
            if let Value::Tuple(pair) = item {
                let pair = pair.borrow();
                headers.push((pair[0].display(), pair[1].display()));
            }
        }
    }
    match do_http(&verb, &url, &headers, body, timeout, agent.as_ref()) {
        Ok((status, text, resp_headers)) => Value::ok(Value::struct_of(
            "HttpResponse",
            [
                ("status".into(), Value::Int(status as i64)),
                ("body".into(), Value::str(text)),
                ("headers".into(), header_pairs(resp_headers)),
            ],
        )),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn header_pairs(pairs: Vec<(String, String)>) -> Value {
    Value::vec(
        pairs
            .into_iter()
            .map(|(k, v)| Value::Tuple(Rc::new(RefCell::new(vec![Value::str(k), Value::str(v)]))))
            .collect(),
    )
}

pub(super) fn do_http(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<String>,
    timeout: Option<std::time::Duration>,
    agent: Option<&Rc<RefCell<Native>>>,
) -> Result<(u16, String, Vec<(String, String)>)> {
    // Build the request through the shared agent when one is given, so its
    // cookie jar carries across calls; otherwise use ureq's free functions.
    let agent = agent.and_then(|h| match &*h.borrow() {
        Native::Agent(a) => Some(a.clone()),
        _ => None,
    });
    let with_body = matches!(method, "POST" | "PUT" | "PATCH");
    if with_body {
        let mut b = match (&agent, method) {
            (Some(a), "POST") => a.post(url),
            (Some(a), "PUT") => a.put(url),
            (Some(a), _) => a.patch(url),
            (None, "POST") => ureq::post(url),
            (None, "PUT") => ureq::put(url),
            (None, _) => ureq::patch(url),
        };
        if let Some(d) = timeout {
            b = b.config().timeout_global(Some(d)).build();
        }
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.send(body.as_deref().unwrap_or(""))?;
        let status = resp.status().as_u16();
        let out_headers = read_headers(&resp);
        Ok((status, resp.body_mut().read_to_string()?, out_headers))
    } else {
        let mut b = match (&agent, method) {
            (Some(a), "DELETE") => a.delete(url),
            (Some(a), "HEAD") => a.head(url),
            (Some(a), _) => a.get(url),
            (None, "DELETE") => ureq::delete(url),
            (None, "HEAD") => ureq::head(url),
            (None, _) => ureq::get(url),
        };
        if let Some(d) = timeout {
            b = b.config().timeout_global(Some(d)).build();
        }
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.call()?;
        let status = resp.status().as_u16();
        let out_headers = read_headers(&resp);
        Ok((status, resp.body_mut().read_to_string()?, out_headers))
    }
}

fn read_headers(resp: &ureq::http::Response<ureq::Body>) -> Vec<(String, String)> {
    resp.headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}

pub(super) fn response_method(s: &StructData, method: &str) -> Value {
    match method {
        "status" => Value::struct_of(
            "StatusCode",
            [("code".into(), s.get("status").unwrap_or(Value::Int(0)))],
        ),
        "body_mut" | "body" | "into_body" => Value::struct_of(
            "HttpBody",
            [("text".into(), s.get("body").unwrap_or_else(|| Value::str("")))],
        ),
        "into_string" => Value::ok(s.get("body").unwrap_or_else(|| Value::str(""))),
        "headers" => Value::struct_of(
            "HeaderMap",
            [("map".into(), s.get("headers").unwrap_or_else(|| Value::vec(vec![])))],
        ),
        _ => Value::Unit,
    }
}

// Header names are case-insensitive, so `get` matches without regard to case
// and hands back a HeaderValue whose `to_str` yields the string.
fn header_map_method(s: &StructData, method: &str, args: &[Value]) -> Value {
    match method {
        "get" => {
            let name = args.first().map(|v| v.display()).unwrap_or_default().to_lowercase();
            if let Some(Value::Vec(h)) = s.get("map") {
                for item in h.borrow().iter() {
                    if let Value::Tuple(pair) = item {
                        let pair = pair.borrow();
                        if pair[0].display().to_lowercase() == name {
                            return Value::some(Value::struct_of(
                                "HeaderValue",
                                [("text".into(), pair[1].clone())],
                            ));
                        }
                    }
                }
            }
            Value::none()
        }
        _ => Value::Unit,
    }
}

fn header_value_method(s: &StructData, method: &str) -> Value {
    let text = s.get("text").map(|v| v.display()).unwrap_or_default();
    match method {
        "to_str" => Value::ok(Value::str(text)),
        "as_str" | "as_string" | "to_string" => Value::str(text),
        _ => Value::Unit,
    }
}

pub(super) fn body_method(s: &StructData, method: &str) -> Result<Value> {
    let text = s.get("text").map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "read_to_string" => Value::ok(Value::str(text)),
        "read_json" => match parse_json(&text) {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        _ => bail!("unknown method `{method}` on a body"),
    })
}

pub(super) fn status_method(s: &StructData, method: &str) -> Value {
    let code = match s.get("code") {
        Some(Value::Int(c)) => c,
        _ => 0,
    };
    match method {
        "as_u16" | "as_int" => Value::Int(code),
        "is_success" => Value::Bool((200..300).contains(&code)),
        "is_client_error" => Value::Bool((400..500).contains(&code)),
        "is_server_error" => Value::Bool((500..600).contains(&code)),
        _ => Value::Unit,
    }
}
