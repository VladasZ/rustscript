//! `Send + Sync` host resources for the parallel engine. The parallel engine
//! grows its native surface as scripts need it. Beyond tasks and futures it now
//! carries the subprocess family, so a `#[tokio::main]` script can spawn a child
//! and stream its pipes from concurrent tasks the same way the fast engine does.

use std::future::Future;
use std::io::{BufReader, Read};
use std::pin::Pin;
use std::process::{Child, ChildStdin};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use super::pvalue::PValue;

/// A boxed future that yields a script value. `Send` so it can be driven on any
/// worker thread.
pub type BoxFut = Pin<Box<dyn Future<Output = PValue> + Send>>;

/// A line iterator over a pipe. `Send` so a lane reading a child can live on a
/// worker thread.
pub type LineIter = Box<dyn Iterator<Item = std::io::Result<String>> + Send>;

pub enum PNative {
    /// A spawned task, joined when awaited.
    Task(tokio::task::JoinHandle<PValue>),
    /// A pending future, for example `tokio::time::sleep` or an async request.
    Future(BoxFut),
    /// An async reqwest client, cheap to clone and shared across tasks.
    HttpClient(reqwest::Client),
    /// A monotonic clock reading used by timed async scripts.
    Instant(Instant),
    /// A spawned child process, waited on through its `Child` value.
    Child(Child),
    /// The writable end of a child's piped stdin.
    ChildStdin(ChildStdin),
    /// A buffered reader over a child's piped stdout or stderr.
    Reader(BufReader<Box<dyn Read + Send>>),
    /// A lazy line iterator, so `for line in reader.lines()` streams a pipe
    /// instead of buffering all of it first.
    Lines(LineIter),
    /// A compiled pattern, shared across tasks so it compiles once.
    Regex(super::pregex::PRegexValue),
    /// A single match, holding its source and byte range.
    RegexMatch(super::pregex::PMatchValue),
    /// A capture set, indexable by group number or name.
    RegexCaptures(super::pregex::PCapturesValue),
    /// A consumed handle, left behind after a task or future is taken to await,
    /// or after a stdin pipe is closed so the child sees EOF.
    Taken,
}

impl PNative {
    pub fn type_name(&self) -> &'static str {
        match self {
            PNative::Task(_) => "JoinHandle",
            PNative::Future(_) => "Future",
            PNative::HttpClient(_) => "Client",
            PNative::Instant(_) => "Instant",
            PNative::Child(_) => "Child",
            PNative::ChildStdin(_) => "ChildStdin",
            PNative::Reader(_) => "Reader",
            PNative::Lines(_) => "Lines",
            PNative::Regex(_) => "Regex",
            PNative::RegexMatch(_) => "Match",
            PNative::RegexCaptures(_) => "Captures",
            PNative::Taken => "Taken",
        }
    }

    /// The readable side of a handle, for the shared reader methods.
    pub fn as_read(&mut self) -> Option<&mut dyn Read> {
        match self {
            PNative::Reader(r) => Some(r),
            _ => None,
        }
    }

    pub fn wrap(self) -> PValue {
        PValue::Native(Arc::new(Mutex::new(self)))
    }
}
