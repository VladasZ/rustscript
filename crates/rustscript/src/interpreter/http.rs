//! The async reqwest bridge. It presents the same
//! script surface for both flavors, `reqwest::get`,
//! `Client`, request builders, and responses, but the network calls return
//! futures so `.send().await` and `.text().await` drive on the tokio runtime.
//!
//! The request and response are modeled as plain structs. Only `.send()`,
//! `.text()`, and `.json()` yield futures, because those are the awaited
//! points in async code.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use reqwest::{Client, Method};

use super::json_bridge::{json_to_pvalue, parse_json, pvalue_to_json};
use super::native::Native;
use super::std_bridge::duration_from_value;
use super::value::{StructData, Value};

/// A shared async client for the `reqwest::get` free function, so a script that
/// fires many one-off gets reuses one connection pool.
fn default_client() -> Client {
    static C: OnceLock<Client> = OnceLock::new();
    C.get_or_init(Client::new).clone()
}

fn client_value(c: Client) -> Value {
    Native::HttpClient(c).wrap()
}

// -- the blocking client ---------------------------------------------------

fn build_blocking_client(
    cookie_store: bool,
    timeout: Option<Duration>,
    ua: Option<String>,
    redirect: Option<reqwest::redirect::Policy>,
) -> Result<reqwest::blocking::Client> {
    let mut b = reqwest::blocking::Client::builder().cookie_store(cookie_store);
    if let Some(d) = timeout {
        b = b.timeout(d);
    }
    if let Some(ua) = ua {
        b = b.user_agent(ua);
    }
    if let Some(policy) = redirect {
        b = b.redirect(policy);
    }
    b.build()
        .map_err(|e| anyhow!("http client build failed: {e}"))
}

/// A shared client for the `reqwest::blocking::get` free function, so a script
/// that fires many one-off gets does not spin up a runtime thread per call.
/// Safe because script code always runs on blocking threads.
fn default_blocking_client() -> Result<reqwest::blocking::Client> {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c.clone());
    }
    let c = build_blocking_client(false, None, None, None)?;
    if C.set(c.clone()).is_err() {
        return C
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("shared HTTP client was not initialized"));
    }
    Ok(c)
}

fn blocking_client_value(c: reqwest::blocking::Client) -> Value {
    Native::BlockingHttpClient(c).wrap()
}

// -- dispatch of `reqwest::..` path calls ----------------------------------

/// Handle a call whose canonical path starts with `reqwest`. Both APIs live
/// here: the blocking one for plain scripts and the async one whose futures
/// drive on the runtime under `.await`.
pub(super) fn reqwest_call(segs: &[String], args: &[Value]) -> Result<Value> {
    let last = segs.last().map_or("", String::as_str);
    // A redirect policy marker, built by `reqwest::redirect::Policy::none()`
    // or `::limited(n)`. It carries no `blocking` segment, so match it first.
    if segs.iter().any(|s| s == "redirect") {
        return Ok(match last {
            "none" => Value::struct_of("RedirectPolicy", [("kind".into(), Value::str("none"))]),
            "limited" => Value::struct_of(
                "RedirectPolicy",
                [
                    ("kind".into(), Value::str("limited")),
                    ("n".into(), args.first().cloned().unwrap_or(Value::Int(10))),
                ],
            ),
            _ => bail!("unsupported redirect policy `{last}`"),
        });
    }
    let blocking = segs.iter().any(|s| s == "blocking");
    if segs.iter().any(|s| s == "Client") {
        return match (blocking, last) {
            (true, "new") => Ok(blocking_client_value(build_blocking_client(
                false, None, None, None,
            )?)),
            (false, "new") => Ok(client_value(Client::new())),
            (true, "builder") => Ok(blocking_builder_value()),
            (false, "builder") => Ok(builder_value()),
            _ => bail!("unknown reqwest Client function `{last}`"),
        };
    }
    // The only free function either API exposes is `get`.
    if last == "get" {
        let url = args.first().map(Value::display).unwrap_or_default();
        if blocking {
            return Ok(run_blocking(&request_struct("GET", &url, Value::Unit)));
        }
        return Ok(send_future(&request_struct("GET", &url, Value::Unit)));
    }
    bail!("unsupported reqwest function `{last}`, build a Client for other verbs")
}

// -- request and builder values --------------------------------------------

fn request_struct(method: &str, url: &str, client: Value) -> Arc<StructData> {
    let v = Value::struct_of(
        "ReqwestRequest",
        [
            ("method".into(), Value::str(method)),
            ("url".into(), Value::str(url)),
            ("headers".into(), Value::vec(vec![])),
            ("query".into(), Value::vec(vec![])),
            ("body".into(), Value::Unit),
            ("timeout".into(), Value::Unit),
            ("client".into(), client),
        ],
    );
    match v {
        Value::Struct(s) => s,
        _ => unreachable!(),
    }
}

fn builder_value() -> Value {
    Value::struct_of(
        "ReqwestClientBuilder",
        [
            ("cookie_store".into(), Value::Bool(false)),
            ("timeout".into(), Value::Unit),
            ("user_agent".into(), Value::Unit),
            ("redirect".into(), Value::Unit),
            ("blocking".into(), Value::Bool(false)),
        ],
    )
}

fn blocking_builder_value() -> Value {
    Value::struct_of(
        "ReqwestClientBuilder",
        [
            ("cookie_store".into(), Value::Bool(false)),
            ("timeout".into(), Value::Unit),
            ("user_agent".into(), Value::Unit),
            ("redirect".into(), Value::Unit),
            ("blocking".into(), Value::Bool(true)),
        ],
    )
}

// Read the redirect policy a builder stashed via `.redirect(..)`. The policy
// marker is built by the `reqwest::redirect::Policy::..` path calls above.
fn redirect_policy(s: &StructData) -> Option<reqwest::redirect::Policy> {
    let Some(Value::Struct(rp)) = s.get("redirect") else {
        return None;
    };
    if &**rp.name() != "RedirectPolicy" {
        return None;
    }
    match rp.get("kind").map(|v| v.display()).as_deref() {
        Some("none") => Some(reqwest::redirect::Policy::none()),
        Some("limited") => {
            let n = match rp.get("n") {
                Some(Value::Int(n)) => usize::try_from(n).unwrap_or(10),
                _ => 10,
            };
            Some(reqwest::redirect::Policy::limited(n))
        }
        _ => None,
    }
}

// -- method dispatch -------------------------------------------------------

/// Route a method on one of the http struct types. Returns `None` when the
/// receiver is not an http type, so the caller can try other dispatch.
pub(super) fn http_method(recv: &Value, method: &str, args: &[Value]) -> Option<Result<Value>> {
    match recv {
        Value::Native(n)
            if matches!(
                &*n.lock(),
                Native::HttpClient(_) | Native::BlockingHttpClient(_)
            ) =>
        {
            Some(client_method(n, method, args))
        }
        Value::Struct(s) => match &**s.name() {
            "ReqwestClientBuilder" => Some(builder_method(s, method, args)),
            "ReqwestRequest" => Some(request_method(s, method, args)),
            "ReqwestResponse" => Some(response_method(s, method)),
            "StatusCode" => Some(Ok(status_method(s, method))),
            "HeaderMap" => Some(Ok(header_map_method(s, method, args))),
            "HeaderValue" => Some(Ok(header_value_method(s, method))),
            _ => None,
        },
        _ => None,
    }
}

fn client_method(
    n: &Arc<parking_lot::Mutex<Native>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let verb = match method {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        "clone" => return Ok(Value::Native(n.clone())),
        _ => bail!("unknown method `{method}` on a client"),
    };
    let url = args.first().map(Value::display).unwrap_or_default();
    Ok(Value::Struct(request_struct(
        verb,
        &url,
        Value::Native(n.clone()),
    )))
}

fn builder_method(s: &Arc<StructData>, method: &str, args: &[Value]) -> Result<Value> {
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
            s.set("timeout", args.first().cloned().unwrap_or(Value::Unit));
            Ok(this())
        }
        "user_agent" => {
            s.set("user_agent", args.first().cloned().unwrap_or(Value::Unit));
            Ok(this())
        }
        "redirect" => {
            s.set("redirect", args.first().cloned().unwrap_or(Value::Unit));
            Ok(this())
        }
        "build" => {
            let cookies = matches!(s.get("cookie_store"), Some(Value::Bool(true)));
            let timeout = duration_field(s, "timeout");
            let ua = match s.get("user_agent") {
                Some(Value::Str(u)) => Some(u.to_string()),
                _ => None,
            };
            if matches!(s.get("blocking"), Some(Value::Bool(true))) {
                return Ok(
                    match build_blocking_client(cookies, timeout, ua, redirect_policy(s)) {
                        Ok(c) => Value::ok(blocking_client_value(c)),
                        Err(e) => Value::err(Value::str(e.to_string())),
                    },
                );
            }
            let mut b = Client::builder().cookie_store(cookies);
            if let Some(d) = timeout {
                b = b.timeout(d);
            }
            if let Some(ua) = ua {
                b = b.user_agent(ua);
            }
            if let Some(policy) = redirect_policy(s) {
                b = b.redirect(policy);
            }
            Ok(match b.build() {
                Ok(c) => Value::ok(client_value(c)),
                Err(e) => Value::err(Value::str(e.to_string())),
            })
        }
        _ => bail!("unknown method `{method}` on a client builder"),
    }
}

fn request_method(s: &Arc<StructData>, method: &str, args: &[Value]) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    match method {
        "header" => {
            let k = args.first().map(Value::display).unwrap_or_default();
            let v = args.get(1).map(Value::display).unwrap_or_default();
            add_header(s, &k, &v);
            Ok(this())
        }
        "bearer_auth" => {
            let token = args.first().map(Value::display).unwrap_or_default();
            add_header(s, "Authorization", &format!("Bearer {token}"));
            Ok(this())
        }
        "basic_auth" => {
            let user = args.first().map(Value::display).unwrap_or_default();
            let pass = match args.get(1) {
                Some(Value::Enum { data, .. }) => {
                    data.first().map(Value::display).unwrap_or_default()
                }
                Some(other) => other.display(),
                None => String::new(),
            };
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            add_header(s, "Authorization", &format!("Basic {token}"));
            Ok(this())
        }
        "query" => {
            if let Some(Value::Vec(items)) = args.first()
                && let Some(Value::Vec(q)) = s.get("query")
            {
                for item in items.lock().iter() {
                    q.lock().push(item.clone());
                }
            }
            Ok(this())
        }
        "json" => {
            let json = pvalue_to_json(args.first().unwrap_or(&Value::Unit))?;
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
            s.set("timeout", args.first().cloned().unwrap_or(Value::Unit));
            Ok(this())
        }
        // A request built from a blocking client runs at once; one built from
        // an async client hands back a future for `.await`.
        "send" => {
            let blocking = matches!(
                s.get("client"),
                Some(Value::Native(n)) if matches!(&*n.lock(), Native::BlockingHttpClient(_))
            );
            if blocking {
                Ok(run_blocking(s))
            } else {
                Ok(send_future(s))
            }
        }
        _ => bail!("unknown method `{method}` on a request"),
    }
}

// -- blocking execution ----------------------------------------------------

fn run_blocking(s: &StructData) -> Value {
    match execute_blocking(s) {
        Ok(v) => Value::ok(v),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn execute_blocking(s: &StructData) -> Result<Value> {
    let method = s
        .get("method")
        .map_or_else(|| "GET".into(), |v| v.display());
    let url = s.get("url").map(|v| v.display()).unwrap_or_default();
    let client = match s.get("client") {
        Some(Value::Native(h)) => match &*h.lock() {
            Native::BlockingHttpClient(c) => c.clone(),
            _ => default_blocking_client()?,
        },
        _ => default_blocking_client()?,
    };
    let m = Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET);
    let mut rb = client.request(m, &url);
    let query = pairs_field(s, "query");
    if !query.is_empty() {
        rb = rb.query(&query);
    }
    for (k, v) in pairs_field(s, "headers") {
        rb = rb.header(&k, &v);
    }
    if let Some(d) = duration_field(s, "timeout") {
        rb = rb.timeout(d);
    }
    if let Some(Value::Str(body)) = s.get("body") {
        rb = rb.body(body.to_string());
    }
    let resp = rb.send()?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Taken before the body is read, because reading it consumes the response.
    let length = resp.content_length();
    let text = resp.text()?;
    Ok(Value::struct_of(
        "ReqwestResponse",
        [
            ("status".into(), Value::Int(i64::from(status))),
            // A blocking body is already decoded text, so `text` and `json`
            // answer directly instead of handing back a future.
            ("body".into(), Value::str(text)),
            ("headers".into(), header_pairs(headers)),
            (
                "content_length".into(),
                match length {
                    Some(n) => Value::some(Value::Int(i64::try_from(n).unwrap_or(0))),
                    None => Value::none(),
                },
            ),
        ],
    ))
}

fn add_header(s: &StructData, k: &str, v: &str) {
    if let Some(Value::Vec(h)) = s.get("headers") {
        h.lock()
            .push(Value::tuple(vec![Value::str(k), Value::str(v)]));
    }
}

// -- execution -------------------------------------------------------------

/// The owned request plan handed to the send future, free of any `!Send` value.
struct Plan {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    body: Option<String>,
    timeout: Option<Duration>,
    client: Client,
}

fn build_plan(s: &StructData) -> Plan {
    let method = s
        .get("method")
        .map_or_else(|| "GET".into(), |v| v.display());
    let client = match s.get("client") {
        Some(Value::Native(n)) => match &*n.lock() {
            Native::HttpClient(c) => c.clone(),
            _ => default_client(),
        },
        _ => default_client(),
    };
    Plan {
        method: Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET),
        url: s.get("url").map(|v| v.display()).unwrap_or_default(),
        headers: pairs_field(s, "headers"),
        query: pairs_field(s, "query"),
        body: match s.get("body") {
            Some(Value::Str(b)) => Some(b.to_string()),
            _ => None,
        },
        timeout: duration_field(s, "timeout"),
        client,
    }
}

fn send_future(s: &StructData) -> Value {
    let plan = build_plan(s);
    Native::Future(Box::pin(async move {
        match run_plan(plan).await {
            Ok(resp) => Value::ok(resp),
            Err(e) => Value::err(Value::str(e.to_string())),
        }
    }))
    .wrap()
}

async fn run_plan(plan: Plan) -> Result<Value> {
    let mut rb = plan.client.request(plan.method, &plan.url);
    if !plan.query.is_empty() {
        rb = rb.query(&plan.query);
    }
    for (k, v) in &plan.headers {
        rb = rb.header(k, v);
    }
    if let Some(d) = plan.timeout {
        rb = rb.timeout(d);
    }
    if let Some(body) = plan.body {
        rb = rb.body(body);
    }
    let resp = rb.send().await?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Taken before the body is read, because reading it consumes the response.
    // A speed test counts these instead of the decoded text, whose length is
    // not the number of bytes that crossed the wire.
    let length = resp.content_length();
    // Kept in wire form. Decoding happens in `text` and `json`, so a script
    // that only reads `content_length` never pays for a UTF-8 conversion.
    let raw = resp.bytes().await?.to_vec();
    Ok(Value::struct_of(
        "ReqwestResponse",
        [
            ("status".into(), Value::Int(i64::from(status))),
            ("body".into(), Native::Body(raw).wrap()),
            ("headers".into(), header_pairs(headers)),
            (
                "content_length".into(),
                match length {
                    Some(n) => Value::some(Value::Int(i64::try_from(n).unwrap_or(0))),
                    None => Value::none(),
                },
            ),
        ],
    ))
}

fn pairs_field(s: &StructData, field: &str) -> Vec<(String, String)> {
    match s.get(field) {
        Some(Value::Vec(items)) => items
            .lock()
            .iter()
            .filter_map(|item| {
                let Value::Tuple(pair) = item else {
                    return None;
                };
                let pair = pair.lock();
                Some((pair[0].display(), pair[1].display()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn header_pairs(pairs: Vec<(String, String)>) -> Value {
    Value::vec(
        pairs
            .into_iter()
            .map(|(k, v)| Value::tuple(vec![Value::str(k), Value::str(v)]))
            .collect(),
    )
}

/// A Duration field or a `Some(Duration)` wrapper as the real std value.
fn duration_field(s: &StructData, field: &str) -> Option<Duration> {
    let v = s.get(field)?;
    if let Value::Enum { data, .. } = &v {
        return data.first().and_then(duration_from_value);
    }
    duration_from_value(&v)
}

// -- response methods ------------------------------------------------------

fn response_method(s: &Arc<StructData>, method: &str) -> Result<Value> {
    let this = || Value::Struct(s.clone());
    let body = || body_bytes(s);
    // A blocking response holds its body as decoded text, so `text` and
    // `json` answer directly. An async response holds wire bytes and hands
    // back futures for `.await`.
    let is_blocking = matches!(s.get("body"), Some(Value::Str(_)));
    Ok(match method {
        "status" => Value::struct_of(
            "StatusCode",
            [("code".into(), s.get("status").unwrap_or(Value::Int(0)))],
        ),
        "text" if is_blocking => {
            Value::ok(Value::str(String::from_utf8_lossy(&body()).into_owned()))
        }
        "json" if is_blocking => {
            let text = String::from_utf8_lossy(&body()).into_owned();
            match parse_json(&text) {
                Ok(v) => Value::ok(v),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        "text" => text_future(body()),
        "json" => json_future(body()),
        "content_length" => s.get("content_length").unwrap_or_else(Value::none),
        "headers" => Value::struct_of(
            "HeaderMap",
            [(
                "map".into(),
                s.get("headers").unwrap_or_else(|| Value::vec(vec![])),
            )],
        ),
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

/// The undecoded body of a response value.
fn body_bytes(s: &StructData) -> Vec<u8> {
    match s.get("body") {
        Some(Value::Native(n)) => match &*n.lock() {
            Native::Body(raw) => raw.clone(),
            _ => Vec::new(),
        },
        Some(other) => other.display().into_bytes(),
        None => Vec::new(),
    }
}

fn text_future(body: Vec<u8>) -> Value {
    Native::Future(Box::pin(async move {
        Value::ok(Value::str(String::from_utf8_lossy(&body).into_owned()))
    }))
    .wrap()
}

fn json_future(body: Vec<u8>) -> Value {
    Native::Future(Box::pin(async move {
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => Value::ok(json_to_pvalue(v)),
            Err(e) => Value::err(Value::str(e.to_string())),
        }
    }))
    .wrap()
}

fn header_map_method(s: &StructData, method: &str, args: &[Value]) -> Value {
    match method {
        "get" => {
            let name = args
                .first()
                .map(Value::display)
                .unwrap_or_default()
                .to_lowercase();
            if let Some(Value::Vec(h)) = s.get("map") {
                for item in h.lock().iter() {
                    if let Value::Tuple(pair) = item {
                        let pair = pair.lock();
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
    match super::shared::header_value_core(method, text) {
        Some(super::shared::HeaderOut::Ok(t)) => Value::ok(Value::str(t)),
        Some(super::shared::HeaderOut::Text(t)) => Value::str(t),
        None => Value::Unit,
    }
}

fn status_method(s: &StructData, method: &str) -> Value {
    let code = match s.get("code") {
        Some(Value::Int(c)) => c,
        _ => 0,
    };
    match super::shared::status_core(method, code) {
        Some(super::shared::StatusOut::Int(i)) => Value::Int(i),
        Some(super::shared::StatusOut::Bool(b)) => Value::Bool(b),
        None => Value::Unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A json null from a response body has to be the same None the two json
    /// parsers make. This one answered Unit, and a `Value::Null` pattern only
    /// matches None, so the same null matched or did not depending on whether
    /// it came from `from_str` or from `resp.json()`.
    #[test]
    fn a_response_null_is_the_none_the_parsers_make() {
        assert!(json_to_pvalue(serde_json::Value::Null).is_none_value());
    }
}
