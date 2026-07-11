//! `Send + Sync` host resources for the parallel engine. Kept intentionally
//! small: the parallel engine grows its native surface as scripts need it. A
//! task join handle and an awaitable future are enough for the first fan-out
//! scripts.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;

use super::pvalue::PValue;

/// A boxed future that yields a script value. `Send` so it can be driven on any
/// worker thread.
pub type BoxFut = Pin<Box<dyn Future<Output = PValue> + Send>>;

pub enum PNative {
    /// A spawned task, joined when awaited.
    Task(tokio::task::JoinHandle<PValue>),
    /// A pending future, for example `tokio::time::sleep` or an async request.
    Future(BoxFut),
    /// An async reqwest client, cheap to clone and shared across tasks.
    HttpClient(reqwest::Client),
    /// A consumed handle, left behind after a task or future is taken to await.
    Taken,
}

impl PNative {
    pub fn type_name(&self) -> &'static str {
        match self {
            PNative::Task(_) => "JoinHandle",
            PNative::Future(_) => "Future",
            PNative::HttpClient(_) => "Client",
            PNative::Taken => "Taken",
        }
    }

    pub fn wrap(self) -> PValue {
        PValue::Native(Arc::new(Mutex::new(self)))
    }
}
