//! The async reqwest bridge for the parallel engine. It presents the same
//! script surface as the fast engine's blocking bridge, `reqwest::get`,
//! `Client`, request builders, and responses, but the network calls return
//! futures so `.send().await` and `.text().await` drive on the tokio runtime.
//!
//! The request and response are modeled as plain structs, exactly like the fast
//! engine, so a script reads the same either way. Only `.send()`, `.text()`, and
//! `.json()` yield futures, because those are the awaited points in async code.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Result, bail};
use reqwest::{Client, Method};

use super::pnative::PNative;
use super::pvalue::{PStructData, PValue};

/// A shared async client for the `reqwest::get` free function, so a script that
/// fires many one-off gets reuses one connection pool.
fn default_client() -> Client {
    static C: OnceLock<Client> = OnceLock::new();
    C.get_or_init(Client::new).clone()
}

fn client_value(c: Client) -> PValue {
    PNative::HttpClient(c).wrap()
}

// -- dispatch of `reqwest::..` path calls ----------------------------------

/// Handle a call whose canonical path starts with `reqwest`.
pub(super) fn reqwest_call(segs: &[String], args: &[PValue]) -> Result<PValue> {
    let last = segs.last().map(String::as_str).unwrap_or("");
    if segs.iter().any(|s| s == "blocking") {
        bail!("use the async reqwest API under #[tokio::main], not reqwest::blocking");
    }
    if segs.iter().any(|s| s == "Client") {
        return match last {
            "new" => Ok(client_value(Client::new())),
            "builder" => Ok(builder_value()),
            _ => bail!("unknown reqwest::Client function `{last}`"),
        };
    }
    if last == "get" {
        let url = args.first().map(PValue::display).unwrap_or_default();
        return Ok(send_future(&request_struct("GET", &url, PValue::Unit)));
    }
    bail!("unsupported reqwest function `{last}`, build a Client for other verbs")
}

// -- request and builder values --------------------------------------------

fn request_struct(method: &str, url: &str, client: PValue) -> Arc<PStructData> {
    let v = PValue::struct_of(
        "ReqwestRequest",
        [
            ("method".into(), PValue::str(method)),
            ("url".into(), PValue::str(url)),
            ("headers".into(), PValue::vec(vec![])),
            ("query".into(), PValue::vec(vec![])),
            ("body".into(), PValue::Unit),
            ("timeout".into(), PValue::Unit),
            ("client".into(), client),
        ],
    );
    match v {
        PValue::Struct(s) => s,
        _ => unreachable!(),
    }
}

fn builder_value() -> PValue {
    PValue::struct_of(
        "ReqwestClientBuilder",
        [
            ("cookie_store".into(), PValue::Bool(false)),
            ("timeout".into(), PValue::Unit),
            ("user_agent".into(), PValue::Unit),
        ],
    )
}

// -- method dispatch -------------------------------------------------------

/// Route a method on one of the http struct types. Returns `None` when the
/// receiver is not an http type, so the caller can try other dispatch.
pub(super) fn http_method(recv: &PValue, method: &str, args: &[PValue]) -> Option<Result<PValue>> {
    match recv {
        PValue::Native(n) if matches!(&*n.lock(), PNative::HttpClient(_)) => {
            Some(client_method(n, method, args))
        }
        PValue::Struct(s) => match &**s.name() {
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
    n: &Arc<parking_lot::Mutex<PNative>>,
    method: &str,
    args: &[PValue],
) -> Result<PValue> {
    let verb = match method {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        _ => bail!("unknown method `{method}` on a client"),
    };
    let url = args.first().map(PValue::display).unwrap_or_default();
    Ok(PValue::Struct(request_struct(
        verb,
        &url,
        PValue::Native(n.clone()),
    )))
}

fn builder_method(s: &Arc<PStructData>, method: &str, args: &[PValue]) -> Result<PValue> {
    let this = || PValue::Struct(s.clone());
    match method {
        "cookie_store" => {
            s.set(
                "cookie_store",
                args.first().cloned().unwrap_or(PValue::Bool(false)),
            );
            Ok(this())
        }
        "timeout" => {
            s.set("timeout", args.first().cloned().unwrap_or(PValue::Unit));
            Ok(this())
        }
        "user_agent" => {
            s.set("user_agent", args.first().cloned().unwrap_or(PValue::Unit));
            Ok(this())
        }
        "build" => {
            let mut b = Client::builder();
            if let Some(d) = duration_field(s, "timeout") {
                b = b.timeout(d);
            }
            if let Some(PValue::Str(ua)) = s.get("user_agent") {
                b = b.user_agent(ua.to_string());
            }
            if matches!(s.get("cookie_store"), Some(PValue::Bool(true))) {
                b = b.cookie_store(true);
            }
            Ok(match b.build() {
                Ok(c) => PValue::ok(client_value(c)),
                Err(e) => PValue::err(PValue::str(e.to_string())),
            })
        }
        _ => bail!("unknown method `{method}` on a client builder"),
    }
}

fn request_method(s: &Arc<PStructData>, method: &str, args: &[PValue]) -> Result<PValue> {
    let this = || PValue::Struct(s.clone());
    match method {
        "header" => {
            let k = args.first().map(PValue::display).unwrap_or_default();
            let v = args.get(1).map(PValue::display).unwrap_or_default();
            add_header(s, &k, &v);
            Ok(this())
        }
        "bearer_auth" => {
            let token = args.first().map(PValue::display).unwrap_or_default();
            add_header(s, "Authorization", &format!("Bearer {token}"));
            Ok(this())
        }
        "basic_auth" => {
            let user = args.first().map(PValue::display).unwrap_or_default();
            let pass = match args.get(1) {
                Some(PValue::Enum { data, .. }) => {
                    data.first().map(PValue::display).unwrap_or_default()
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
            if let Some(PValue::Vec(items)) = args.first()
                && let Some(PValue::Vec(q)) = s.get("query")
            {
                for item in items.lock().iter() {
                    q.lock().push(item.clone());
                }
            }
            Ok(this())
        }
        "json" => {
            let json = pvalue_to_json(args.first().unwrap_or(&PValue::Unit))?;
            add_header(s, "Content-Type", "application/json");
            s.set("body", PValue::str(serde_json::to_string(&json)?));
            Ok(this())
        }
        "body" => {
            s.set(
                "body",
                PValue::str(args.first().map(PValue::display).unwrap_or_default()),
            );
            Ok(this())
        }
        "timeout" => {
            s.set("timeout", args.first().cloned().unwrap_or(PValue::Unit));
            Ok(this())
        }
        "send" => Ok(send_future(s)),
        _ => bail!("unknown method `{method}` on a request"),
    }
}

fn add_header(s: &PStructData, k: &str, v: &str) {
    if let Some(PValue::Vec(h)) = s.get("headers") {
        h.lock()
            .push(PValue::tuple(vec![PValue::str(k), PValue::str(v)]));
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

fn build_plan(s: &PStructData) -> Plan {
    let method = s
        .get("method")
        .map(|v| v.display())
        .unwrap_or_else(|| "GET".into());
    let client = match s.get("client") {
        Some(PValue::Native(n)) => match &*n.lock() {
            PNative::HttpClient(c) => c.clone(),
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
            Some(PValue::Str(b)) => Some(b.to_string()),
            _ => None,
        },
        timeout: duration_field(s, "timeout"),
        client,
    }
}

fn send_future(s: &PStructData) -> PValue {
    let plan = build_plan(s);
    PNative::Future(Box::pin(async move {
        match run_plan(plan).await {
            Ok(resp) => PValue::ok(resp),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    }))
    .wrap()
}

async fn run_plan(plan: Plan) -> Result<PValue> {
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
    let text = resp.text().await?;
    Ok(PValue::struct_of(
        "ReqwestResponse",
        [
            ("status".into(), PValue::Int(status as i64)),
            ("body".into(), PValue::str(text)),
            ("headers".into(), header_pairs(headers)),
        ],
    ))
}

fn pairs_field(s: &PStructData, field: &str) -> Vec<(String, String)> {
    match s.get(field) {
        Some(PValue::Vec(items)) => items
            .lock()
            .iter()
            .filter_map(|item| {
                let PValue::Tuple(pair) = item else {
                    return None;
                };
                let pair = pair.lock();
                Some((pair[0].display(), pair[1].display()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn header_pairs(pairs: Vec<(String, String)>) -> PValue {
    PValue::vec(
        pairs
            .into_iter()
            .map(|(k, v)| PValue::tuple(vec![PValue::str(k), PValue::str(v)]))
            .collect(),
    )
}

/// A Duration field is modeled as a struct with a `millis` field.
fn duration_field(s: &PStructData, field: &str) -> Option<Duration> {
    match s.get(field) {
        Some(PValue::Struct(d)) => match d.get("millis") {
            Some(PValue::Int(m)) if m > 0 => Some(Duration::from_millis(m as u64)),
            _ => None,
        },
        _ => None,
    }
}

// -- response methods ------------------------------------------------------

fn response_method(s: &Arc<PStructData>, method: &str) -> Result<PValue> {
    let this = || PValue::Struct(s.clone());
    let body = || s.get("body").map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "status" => PValue::struct_of(
            "StatusCode",
            [("code".into(), s.get("status").unwrap_or(PValue::Int(0)))],
        ),
        "text" => text_future(body()),
        "json" => json_future(body()),
        "headers" => PValue::struct_of(
            "HeaderMap",
            [(
                "map".into(),
                s.get("headers").unwrap_or_else(|| PValue::vec(vec![])),
            )],
        ),
        "error_for_status" => {
            let code = match s.get("status") {
                Some(PValue::Int(c)) => c,
                _ => 0,
            };
            if (200..400).contains(&code) {
                PValue::ok(this())
            } else {
                PValue::err(PValue::str(format!("HTTP status {code}")))
            }
        }
        _ => bail!("unknown method `{method}` on a response"),
    })
}

fn text_future(body: String) -> PValue {
    PNative::Future(Box::pin(async move { PValue::ok(PValue::str(body)) })).wrap()
}

fn json_future(body: String) -> PValue {
    PNative::Future(Box::pin(async move {
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(v) => PValue::ok(json_to_pvalue(v)),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    }))
    .wrap()
}

fn header_map_method(s: &PStructData, method: &str, args: &[PValue]) -> PValue {
    match method {
        "get" => {
            let name = args
                .first()
                .map(PValue::display)
                .unwrap_or_default()
                .to_lowercase();
            if let Some(PValue::Vec(h)) = s.get("map") {
                for item in h.lock().iter() {
                    if let PValue::Tuple(pair) = item {
                        let pair = pair.lock();
                        if pair[0].display().to_lowercase() == name {
                            return PValue::some(PValue::struct_of(
                                "HeaderValue",
                                [("text".into(), pair[1].clone())],
                            ));
                        }
                    }
                }
            }
            PValue::none()
        }
        _ => PValue::Unit,
    }
}

fn header_value_method(s: &PStructData, method: &str) -> PValue {
    let text = s.get("text").map(|v| v.display()).unwrap_or_default();
    match method {
        "to_str" => PValue::ok(PValue::str(text)),
        "as_str" | "to_string" => PValue::str(text),
        _ => PValue::Unit,
    }
}

fn status_method(s: &PStructData, method: &str) -> PValue {
    let code = match s.get("code") {
        Some(PValue::Int(c)) => c,
        _ => 0,
    };
    match method {
        "as_u16" | "as_int" => PValue::Int(code),
        "is_success" => PValue::Bool((200..300).contains(&code)),
        "is_client_error" => PValue::Bool((400..500).contains(&code)),
        "is_server_error" => PValue::Bool((500..600).contains(&code)),
        _ => PValue::Unit,
    }
}

// -- json conversion -------------------------------------------------------

fn json_to_pvalue(v: serde_json::Value) -> PValue {
    match v {
        serde_json::Value::Null => PValue::Unit,
        serde_json::Value::Bool(b) => PValue::Bool(b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => PValue::Int(i),
            None => PValue::Float(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => PValue::str(s),
        serde_json::Value::Array(items) => {
            PValue::vec(items.into_iter().map(json_to_pvalue).collect())
        }
        serde_json::Value::Object(map) => {
            let m = PValue::map();
            if let PValue::Map(inner) = &m {
                let mut inner = inner.lock();
                for (k, v) in map {
                    inner.insert(
                        super::pvalue::PKey::Str(Arc::from(k.as_str())),
                        json_to_pvalue(v),
                    );
                }
            }
            m
        }
    }
}

fn pvalue_to_json(v: &PValue) -> Result<serde_json::Value> {
    Ok(match v {
        PValue::Unit => serde_json::Value::Null,
        PValue::Bool(b) => serde_json::Value::Bool(*b),
        PValue::Int(i) => serde_json::Value::Number((*i).into()),
        PValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        PValue::Str(s) => serde_json::Value::String(s.to_string()),
        PValue::Vec(items) => {
            let items = items.lock();
            let mut out = Vec::with_capacity(items.len());
            for it in items.iter() {
                out.push(pvalue_to_json(it)?);
            }
            serde_json::Value::Array(out)
        }
        PValue::Map(map) => {
            let map = map.lock();
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map.iter() {
                out.insert(k.to_value().display(), pvalue_to_json(val)?);
            }
            serde_json::Value::Object(out)
        }
        PValue::Struct(s) => {
            let values = s.values.lock();
            let mut out = serde_json::Map::with_capacity(values.len());
            for (k, val) in s.shape.fields.iter().zip(values.iter()) {
                out.insert(k.to_string(), pvalue_to_json(val)?);
            }
            serde_json::Value::Object(out)
        }
        other => bail!("cannot serialize a {} to json", other.type_name()),
    })
}
