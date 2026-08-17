//! `Send + Sync` host resources. The interpreter
//! grows its native surface as scripts need it. Beyond tasks and futures it now
//! carries the subprocess family, so a `#[tokio::main]` script can spawn a child
//! and stream its pipes from concurrent tasks.

use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::pin::Pin;
use std::process::{Child, ChildStdin};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use parking_lot::Mutex;

use super::value::Value;

/// A boxed future that yields a script value. `Send` so it can be driven on any
/// worker thread.
pub type BoxFut = Pin<Box<dyn Future<Output = Value> + Send>>;

/// A line iterator over a pipe. `Send` so a lane reading a child can live on a
/// worker thread.
pub type LineIter = Box<dyn Iterator<Item = std::io::Result<String>> + Send>;

pub enum Native {
    /// A spawned task, joined when awaited.
    Task(tokio::task::JoinHandle<Value>),
    /// A pending future, for example `tokio::time::sleep` or an async request.
    Future(BoxFut),
    /// An async reqwest client, cheap to clone and shared across tasks.
    HttpClient(reqwest::Client),
    /// The blocking reqwest client. Safe here because script code always runs
    /// on blocking threads, never on a runtime worker.
    BlockingHttpClient(reqwest::blocking::Client),
    /// A monotonic clock reading used by timed async scripts.
    Instant(Instant),
    /// A wall clock reading, `SystemTime::now` or a file timestamp.
    SystemTime(SystemTime),
    /// A spawned child process, waited on through its `Child` value.
    Child(Child),
    /// The writable end of a child's piped stdin.
    ChildStdin(ChildStdin),
    /// An open file, buffered, which can also write and seek.
    File(BufReader<File>),
    /// A buffered reader over a child's piped stdout or stderr.
    Reader(BufReader<Box<dyn Read + Send>>),
    /// A writer: stdout, stderr, or another byte sink.
    Writer(Box<dyn Write + Send>),
    /// A bound TCP listener.
    Listener(TcpListener),
    /// A connected TCP stream.
    Stream(TcpStream),
    /// A bound UDP socket.
    Udp(UdpSocket),
    /// A loaded PDF document, the real lopdf value.
    Pdf(Box<lopdf::Document>),
    /// A temporary directory, deleted when the value drops or on `close`.
    TempDir(tempfile::TempDir),
    /// A named temporary file.
    NamedTempFile(tempfile::NamedTempFile),
    /// An in-progress SHA-256 hasher, fed by `update` and read by `finalize`.
    Sha256(sha2::Sha256),
    /// A lazy line iterator, so `for line in reader.lines()` streams a pipe
    /// instead of buffering all of it first.
    Lines(LineIter),
    /// A response body still in its wire form. Kept undecoded so a script that
    /// only wants the byte count never pays for a UTF-8 conversion, which on a
    /// binary payload both costs time and inflates the result.
    Body(Vec<u8>),
    /// A compiled pattern, shared across tasks so it compiles once.
    Regex(super::regex_bridge::RegexValue),
    /// A single match, holding its source and byte range.
    RegexMatch(super::regex_bridge::MatchValue),
    /// A capture set, indexable by group number or name.
    RegexCaptures(super::regex_bridge::CapturesValue),
    /// A lazy iterator, shared like every other handle so `by_ref` and
    /// `peekable` keep their real semantics.
    Iterator(super::iterator::IteratorState),
    /// An `std::io::Error` as scripts observe it: real `Display` and
    /// `Debug` text captured at conversion, plus the kind and code its
    /// accessor methods answer.
    IoErr {
        display: String,
        debug: String,
        kind: String,
        code: Option<i32>,
    },
    /// A tokio `JoinError` as scripts observe it: real `Display` and
    /// `Debug` text captured at conversion, plus what its accessors answer.
    JoinErr {
        display: String,
        debug: String,
        is_panic: bool,
    },
    /// The buffer behind a `fmt::Formatter` handed to a user `fmt` impl.
    /// `write!` into it appends here, and the formatter reads it back as the
    /// rendered text.
    Fmt(String),
    /// A consumed handle, left behind after a task or future is taken to await,
    /// or after a stdin pipe is closed so the child sees EOF.
    Taken,
}

impl Native {
    pub fn type_name(&self) -> &'static str {
        match self {
            Native::Task(_) => "JoinHandle",
            Native::Future(_) => "Future",
            Native::HttpClient(_) | Native::BlockingHttpClient(_) => "Client",
            Native::Instant(_) => "Instant",
            Native::SystemTime(_) => "SystemTime",
            Native::Child(_) => "Child",
            Native::ChildStdin(_) => "ChildStdin",
            Native::File(_) => "File",
            Native::Reader(_) => "Reader",
            Native::Writer(_) => "Writer",
            Native::Listener(_) => "TcpListener",
            Native::Stream(_) => "TcpStream",
            Native::Udp(_) => "UdpSocket",
            Native::Pdf(_) => "PdfDocument",
            Native::TempDir(_) => "TempDir",
            Native::NamedTempFile(_) => "NamedTempFile",
            Native::Sha256(_) => "Sha256",
            Native::Lines(_) => "Lines",
            Native::Body(_) => "Body",
            Native::Regex(_) => "Regex",
            Native::RegexMatch(_) => "Match",
            Native::RegexCaptures(_) => "Captures",
            Native::Iterator(_) => "Iterator",
            Native::IoErr { .. } => "IoError",
            Native::JoinErr { .. } => "JoinError",
            Native::Fmt(_) => "Formatter",
            Native::Taken => "Taken",
        }
    }

    /// The readable side of a handle, for the shared reader methods.
    pub fn as_read(&mut self) -> Option<&mut dyn Read> {
        match self {
            Native::File(r) => Some(r),
            Native::Reader(r) => Some(r),
            _ => None,
        }
    }

    /// The buffered side of a handle, for the reader methods that need a
    /// delimiter. The File and Reader variants already own a `BufReader`, so this
    /// hands out that buffer instead of wrapping a second one around it, which
    /// would eat bytes the next call expects to still be there.
    pub fn as_buf_read(&mut self) -> Option<&mut dyn BufRead> {
        match self {
            Native::File(r) => Some(r),
            Native::Reader(r) => Some(r),
            _ => None,
        }
    }

    pub fn wrap(self) -> Value {
        Value::Native(Arc::new(Mutex::new(self)))
    }
}

/// An io error as the structured value scripts observe. Both format forms
/// are captured from the real error, so `{:?}` prints the exact
/// `Os { code: 2, kind: NotFound, .. }` shape compiled Rust prints.
pub(super) fn io_error_value(e: &std::io::Error) -> Value {
    Native::IoErr {
        display: e.to_string(),
        debug: format!("{e:?}"),
        kind: format!("{:?}", e.kind()),
        code: e.raw_os_error(),
    }
    .wrap()
}
