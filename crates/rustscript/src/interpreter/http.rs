//! The reqwest backed http bridge for the fast engine. Presents the blocking
//! `reqwest` API to scripts: `reqwest::blocking::get`, `Client`, request
//! builders, and responses. The parallel engine has its own async bridge.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::{Result, anyhow, bail};
use reqwest::Method;
use reqwest::blocking::Client;

use super::native::Native;
use super::value::{StructData, Value};

use super::json_bridge::*;
use super::std_bridge::*;

type ResponseParts = (u16, String, Vec<(String, String)>);

// -- client construction ---------------------------------------------------

fn build_client(
    cookie_store: bool,
    timeout: Option<std::time::Duration>,
    ua: Option<String>,
) -> Result<Client> {
    let mut b = Client::builder().cookie_store(cookie_store);
    if let Some(d) = timeout {
        b = b.timeout(d);
    }
    if let Some(ua) = ua {
        b = b.user_agent(ua);
    }
    b.build()
        .map_err(|e| anyhow!("http client build failed: {e}"))
}

/// A shared client for the `reqwest::blocking::get` free function, so a script
/// that fires many one-off gets does not spin up a runtime thread per call.
fn default_client() -> Result<Client> {
    static C: OnceLock<Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c.clone());
    }
    let c = build_client(false, None, None)?;
    if C.set(c.clone()).is_err() {
        return C
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("shared HTTP client was not initialized"));
    }
    Ok(c)
}

fn client_value(c: Client) -> Value {
    Native::HttpClient(c).wrap()
}

// -- dispatch of `reqwest::..` path calls ----------------------------------

/// Handle a call whose canonical path starts with `reqwest`. Only the blocking
/// API runs here; the async API is served by the parallel engine.
pub(super) fn reqwest_call(canon: &[String], args: &[Value]) -> Result<Value> {
    if !canon.iter().any(|s| s == "blocking") {
        bail!("async reqwest needs #[tokio::main]; use reqwest::blocking in a plain script");
    }
    let last = canon.last().map(String::as_str).unwrap_or("");
    if canon.iter().any(|s| s == "Client") {
        return match last {
            "new" => Ok(client_value(build_client(false, None, None)?)),
            "builder" => Ok(builder_value()),
            _ => bail!("unknown reqwest::blocking::Client function `{last}`"),
        };
    }
    // The only free function reqwest::blocking exposes is `get`, which runs the
    // request at once and returns a Result<Response>.
    if last == "get" {
        let url = args.first().map(Value::display).unwrap_or_default();
        let req = new_request("GET", &url, None);
        let Value::Struct(s) = &req else {
            unreachable!()
        };
        return Ok(run_request(s));
    }
    bail!("unsupported reqwest::blocking function `{last}`, build a Client for other verbs")
}

// -- request builder value -------------------------------------------------

/// Build a lazy request bound to an optional client handle. Executes only when
/// `.send()` is called, matching reqwest's `RequestBuilder`.
fn new_request(method: &str, url: &str, client: Option<Value>) -> Value {
    Value::struct_of(
        "ReqwestRequest",
        [
            ("method".into(), Value::str(method)),
            ("url".into(), Value::str(url)),
            ("headers".into(), Value::vec(vec![])),
            ("query".into(), Value::Unit),
            ("body".into(), Value::Unit),
            ("timeout".into(), Value::Unit),
            ("client".into(), client.unwrap_or(Value::Unit)),
        ],
    )
}

/// Public helper for `client.get(url)` built in the native method dispatch.
pub(super) fn build_reqwest_request(method: &str, url: Option<&Value>, client: Value) -> Value {
    let url = url.map(Value::display).unwrap_or_default();
    new_request(method, &url, Some(client))
}

fn builder_value() -> Value {
    Value::struct_of(
        "ReqwestClientBuilder",
        [
            ("cookie_store".into(), Value::Bool(false)),
            ("timeout".into(), Value::Unit),
            ("user_agent".into(), Value::Unit),
        ],
    )
}

// -- method dispatch -------------------------------------------------------

pub(super) fn http_method(s: &Rc<StructData>, method: &str, args: &[Value]) -> Result<Value> {
    match &**s.name() {
        "ReqwestClientBuilder" => builder_method(s, method, args),
        "ReqwestRequest" => request_method(s, method, args),
        "ReqwestResponse" => response_method(s, method),
        "StatusCode" => Ok(status_method(s, method)),
        "HeaderMap" => Ok(header_map_method(s, method, args)),
        "HeaderValue" => Ok(header_value_method(s, method)),
        _ => bail!("unknown http method `{method}`"),
    }
}

fn builder_method(s: &Rc<StructData>, method: &str, args: &[Value]) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    match method {
        "cookie_store" => {
            s.set(
                "cookie_store",
                args.first().cloned().unwrap_or(Value::Bool(false)),
            );
            Ok(this())
        }
        "timeout" => {
            if let Some(d) = duration_arg(args.first()) {
                s.set("timeout", d);
            }
            Ok(this())
        }
        "user_agent" => {
            s.set("user_agent", args.first().cloned().unwrap_or(Value::Unit));
            Ok(this())
        }
        "build" => {
            let cookie_store = matches!(s.get("cookie_store"), Some(Value::Bool(true)));
            let timeout = s.get("timeout").and_then(|v| duration_from_value(&v));
            let ua = match s.get("user_agent") {
                Some(Value::Str(u)) => Some(u.as_str().to_string()),
                _ => None,
            };
            Ok(match build_client(cookie_store, timeout, ua) {
                Ok(c) => Value::ok(client_value(c)),
                Err(e) => Value::err(Value::str(e.to_string())),
            })
        }
        _ => bail!("unknown method `{method}` on a client builder"),
    }
}

pub(super) fn request_method(s: &Rc<StructData>, method: &str, args: &[Value]) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    match method {
        "header" => {
            push_pair(s, "headers", args.first(), args.get(1));
            Ok(this())
        }
        "bearer_auth" => {
            let token = args.first().map(Value::display).unwrap_or_default();
            add_header(s, "Authorization", &format!("Bearer {token}"));
            Ok(this())
        }
        "basic_auth" => {
            let user = args.first().map(Value::display).unwrap_or_default();
            // The password is an Option in reqwest, so unwrap a Some payload.
            let pass = match args.get(1) {
                Some(Value::Enum { data, .. }) => {
                    data.first().map(Value::display).unwrap_or_default()
                }
                Some(other) => other.display(),
                None => String::new(),
            };
            use base64::Engine;
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            add_header(s, "Authorization", &format!("Basic {token}"));
            Ok(this())
        }
        "query" => {
            // reqwest takes a slice of pairs, so the argument is a vec of tuples.
            if let Some(Value::Vec(items)) = args.first() {
                let q = ensure_vec_field(s, "query");
                for item in items.borrow().iter() {
                    q.borrow_mut().push(item.clone());
                }
            }
            Ok(this())
        }
        "json" => {
            let json = value_to_json(args.first().unwrap_or(&Value::Unit))?;
            add_header(s, "Content-Type", "application/json");
            s.set("body", Value::str(serde_json::to_string(&json)?));
            Ok(this())
        }
        "body" => {
            s.set(
                "body",
                Value::str(args.first().map(Value::display).unwrap_or_default()),
            );
            Ok(this())
        }
        "timeout" => {
            if let Some(d) = duration_arg(args.first()) {
                s.set("timeout", d);
            }
            Ok(this())
        }
        "send" => Ok(run_request(s)),
        _ => bail!("unknown method `{method}` on a request"),
    }
}

fn push_pair(s: &StructData, field: &str, k: Option<&Value>, v: Option<&Value>) {
    let pair = vec![
        k.cloned().unwrap_or(Value::Unit),
        v.cloned().unwrap_or(Value::Unit),
    ];
    ensure_vec_field(s, field)
        .borrow_mut()
        .push(Value::Tuple(Rc::new(RefCell::new(pair))));
}

fn add_header(s: &StructData, k: &str, v: &str) {
    ensure_vec_field(s, "headers")
        .borrow_mut()
        .push(Value::Tuple(Rc::new(RefCell::new(vec![
            Value::str(k),
            Value::str(v),
        ]))));
}

fn ensure_vec_field(s: &StructData, field: &str) -> Rc<RefCell<Vec<Value>>> {
    match s.get(field) {
        Some(Value::Vec(v)) => v,
        _ => {
            let v = Rc::new(RefCell::new(vec![]));
            s.set(field, Value::Vec(v.clone()));
            v
        }
    }
}

/// A Duration argument may arrive bare or wrapped in an Option.
fn duration_arg(v: Option<&Value>) -> Option<Value> {
    match v {
        Some(Value::Enum { data, .. }) => data.first().cloned(),
        other => other.cloned(),
    }
}

// -- execution -------------------------------------------------------------

fn run_request(s: &StructData) -> Value {
    match execute(s) {
        Ok((status, text, headers)) => Value::ok(Value::struct_of(
            "ReqwestResponse",
            [
                ("status".into(), Value::Int(status as i64)),
                ("body".into(), Value::str(text)),
                ("headers".into(), header_pairs(headers)),
            ],
        )),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn execute(s: &StructData) -> Result<ResponseParts> {
    let method = s
        .get("method")
        .map(|v| v.display())
        .unwrap_or_else(|| "GET".into());
    let url = s.get("url").map(|v| v.display()).unwrap_or_default();
    let client = match s.get("client") {
        Some(Value::Native(h)) => match &*h.borrow() {
            Native::HttpClient(c) => c.clone(),
            _ => default_client()?,
        },
        _ => default_client()?,
    };
    let m = Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET);
    let mut rb = client.request(m, &url);
    if let Some(Value::Vec(q)) = s.get("query") {
        let pairs = tuple_pairs(&q.borrow());
        if !pairs.is_empty() {
            rb = rb.query(&pairs);
        }
    }
    if let Some(Value::Vec(h)) = s.get("headers") {
        for (k, v) in tuple_pairs(&h.borrow()) {
            rb = rb.header(&k, &v);
        }
    }
    if let Some(d) = s.get("timeout").and_then(|v| duration_from_value(&v)) {
        rb = rb.timeout(d);
    }
    if let Some(Value::Str(body)) = s.get("body") {
        rb = rb.body(body.as_str().to_string());
    }
    let resp = rb.send()?;
    let status = resp.status().as_u16();
    let out_headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let text = resp.text()?;
    Ok((status, text, out_headers))
}

fn tuple_pairs(items: &[Value]) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|item| {
            let Value::Tuple(pair) = item else {
                return None;
            };
            let pair = pair.borrow();
            Some((pair[0].display(), pair[1].display()))
        })
        .collect()
}

fn header_pairs(pairs: Vec<(String, String)>) -> Value {
    Value::vec(
        pairs
            .into_iter()
            .map(|(k, v)| Value::Tuple(Rc::new(RefCell::new(vec![Value::str(k), Value::str(v)]))))
            .collect(),
    )
}

// -- response methods ------------------------------------------------------

pub(super) fn response_method(s: &Rc<StructData>, method: &str) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    let body = || s.get("body").map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "status" => Value::struct_of(
            "StatusCode",
            [("code".into(), s.get("status").unwrap_or(Value::Int(0)))],
        ),
        "text" => Value::ok(Value::str(body())),
        "json" => match parse_json(&body()) {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        "headers" => Value::struct_of(
            "HeaderMap",
            [(
                "map".into(),
                s.get("headers").unwrap_or_else(|| Value::vec(vec![])),
            )],
        ),
        // Return the response itself on a 2xx, an Err carrying the status code
        // otherwise, matching reqwest's error_for_status.
        "error_for_status" => {
            let code = match s.get("status") {
                Some(Value::Int(c)) => c,
                _ => 0,
            };
            if (200..400).contains(&code) {
                Value::ok(this())
            } else {
                Value::err(Value::str(format!("HTTP status {code}")))
            }
        }
        _ => bail!("unknown method `{method}` on a response"),
    })
}

// Header names are case-insensitive, so `get` matches without regard to case
// and hands back a HeaderValue whose `to_str` yields the string.
fn header_map_method(s: &StructData, method: &str, args: &[Value]) -> Value {
    match method {
        "get" => {
            let name = args
                .first()
                .map(|v| v.display())
                .unwrap_or_default()
                .to_lowercase();
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
