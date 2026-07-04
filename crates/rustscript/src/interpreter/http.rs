//! The ureq backed http bridge. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::native::Native;

use super::value::{Fields, Value};

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
/// config carry through the call.
pub(super) fn build_http_request(method: &str, url: Option<&Value>, agent: Option<Value>) -> Value {
    let mut fields = Fields::default();
    fields.insert("method".into(), Value::str(method));
    fields.insert(
        "url".into(),
        Value::str(url.map(|v| v.display()).unwrap_or_default()),
    );
    fields.insert("headers".into(), Value::vec(vec![]));
    if let Some(a) = agent {
        fields.insert("agent".into(), a);
    }
    Value::Struct {
        name: "HttpRequest".into(),
        fields: Rc::new(RefCell::new(fields)),
    }
}

pub(super) fn http_method(
    struct_name: &str,
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    match struct_name {
        "HttpRequest" => request_method(fields, method, args),
        "HttpResponse" => Ok(response_method(fields, method)),
        "HttpBody" => body_method(fields, method),
        "StatusCode" => Ok(status_method(fields, method)),
        _ => bail!("unknown http method `{method}`"),
    }
}

pub(super) fn request_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let this = || Value::Struct {
        name: "HttpRequest".into(),
        fields: fields.clone(),
    };
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
            if let Some(Value::Vec(h)) = fields.borrow().get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            }
            Ok(this())
        }
        "call" => Ok(run_request(fields, None)),
        "send" | "send_string" => {
            let body = args.first().map(|v| v.display()).unwrap_or_default();
            Ok(run_request(fields, Some(body)))
        }
        "send_json" => {
            let json = value_to_json(args.first().unwrap_or(&Value::Unit))?;
            if let Some(Value::Vec(h)) = fields.borrow().get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(vec![
                    Value::str("Content-Type"),
                    Value::str("application/json"),
                ]))));
            }
            Ok(run_request(fields, Some(serde_json::to_string(&json)?)))
        }
        "query" => {
            let pair = vec![
                args.first().cloned().unwrap_or(Value::Unit),
                args.get(1).cloned().unwrap_or(Value::Unit),
            ];
            let mut f = fields.borrow_mut();
            let entry = f.entry("query".into()).or_insert_with(|| Value::vec(vec![]));
            if let Value::Vec(q) = entry {
                q.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            }
            drop(f);
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
                fields.borrow_mut().insert("timeout".into(), d);
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

pub(super) fn run_request(fields: &Rc<RefCell<Fields>>, body: Option<String>) -> Value {
    let f = fields.borrow();
    let verb = f.get("method").map(|v| v.display()).unwrap_or_else(|| "GET".into());
    let mut url = f.get("url").map(|v| v.display()).unwrap_or_default();
    // Append any query parameters onto the URL.
    if let Some(Value::Vec(q)) = f.get("query") {
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
    let timeout = f.get("timeout").and_then(duration_from_value);
    let agent = match f.get("agent") {
        Some(Value::Native(h)) => Some(h.clone()),
        _ => None,
    };
    let mut headers = Vec::new();
    if let Some(Value::Vec(h)) = f.get("headers") {
        for item in h.borrow().iter() {
            if let Value::Tuple(pair) = item {
                let pair = pair.borrow();
                headers.push((pair[0].display(), pair[1].display()));
            }
        }
    }
    match do_http(&verb, &url, &headers, body, timeout, agent.as_ref()) {
        Ok((status, text)) => {
            let mut rf = Fields::default();
            rf.insert("status".into(), Value::Int(status as i64));
            rf.insert("body".into(), Value::str(text));
            Value::ok(Value::Struct {
                name: "HttpResponse".into(),
                fields: Rc::new(RefCell::new(rf)),
            })
        }
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn do_http(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<String>,
    timeout: Option<std::time::Duration>,
    agent: Option<&Rc<RefCell<Native>>>,
) -> Result<(u16, String)> {
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
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
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
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
    }
}

pub(super) fn response_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Value {
    let f = fields.borrow();
    match method {
        "status" => {
            let mut sf = Fields::default();
            sf.insert("code".into(), f.get("status").cloned().unwrap_or(Value::Int(0)));
            Value::Struct {
                name: "StatusCode".into(),
                fields: Rc::new(RefCell::new(sf)),
            }
        }
        "body_mut" | "body" | "into_body" => {
            let mut bf = Fields::default();
            bf.insert("text".into(), f.get("body").cloned().unwrap_or_else(|| Value::str("")));
            Value::Struct {
                name: "HttpBody".into(),
                fields: Rc::new(RefCell::new(bf)),
            }
        }
        "into_string" => Value::ok(f.get("body").cloned().unwrap_or_else(|| Value::str(""))),
        _ => Value::Unit,
    }
}

pub(super) fn body_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Result<Value> {
    let text = fields.borrow().get("text").map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "read_to_string" => Value::ok(Value::str(text)),
        "read_json" => match parse_json(&text) {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        _ => bail!("unknown method `{method}` on a body"),
    })
}

pub(super) fn status_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Value {
    let code = match fields.borrow().get("code") {
        Some(Value::Int(c)) => *c,
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

