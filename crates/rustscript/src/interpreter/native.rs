//! `Send + Sync` host resources.

use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::pin::Pin;
use std::process::{Child, ChildStdin};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use parking_lot::Mutex;

use super::value::{Map, MapKey, Value};

/// `Send` so it can be driven on any worker thread.
pub type BoxFut = Pin<Box<dyn Future<Output = Value> + Send>>;

/// `Send` so a lane reading a child can live on a worker thread.
pub type LineIter = Box<dyn Iterator<Item = std::io::Result<String>> + Send>;

pub enum Native {
    /// a `HashMap::entry` handle
    Entry {
        map: Map,
        key: MapKey,
    },
    Task(tokio::task::JoinHandle<Value>),
    Future(BoxFut),
    HttpClient(reqwest::Client),
    /// safe because script code always runs on blocking threads, never on a runtime worker
    BlockingHttpClient(reqwest::blocking::Client),
    Instant(Instant),
    SystemTime(SystemTime),
    Child(Child),
    ChildStdin(ChildStdin),
    File(BufReader<File>),
    Reader(BufReader<Box<dyn Read + Send>>),
    Writer(Box<dyn Write + Send>),
    Listener(TcpListener),
    Stream(TcpStream),
    Udp(UdpSocket),
    /// the real lopdf value
    Pdf(Box<lopdf::Document>),
    /// deleted when the value drops or on `close`
    TempDir(tempfile::TempDir),
    NamedTempFile(tempfile::NamedTempFile),
    Sha256(sha2::Sha256),
    /// lazy, so `for line in reader.lines()` streams a pipe
    Lines(LineIter),
    /// kept undecoded so a script that only wants the byte count never pays for a UTF-8 conversion
    Body(Vec<u8>),
    /// shared across tasks so it compiles once
    Regex(super::regex_bridge::RegexValue),
    RegexMatch(super::regex_bridge::MatchValue),
    RegexCaptures(super::regex_bridge::CapturesValue),
    /// shared like every other handle so `by_ref` and `peekable` keep their real semantics
    Iterator(super::iterator::IteratorState),
    /// real `Display` and `Debug` text captured at conversion, plus the kind and code
    IoErr {
        display: String,
        debug: String,
        kind: String,
        code: Option<i32>,
    },
    /// a parse error with its real `Display` and `Debug` texts
    ParseErr {
        display: String,
        debug: String,
    },
    /// a `JoinError` with its real `Display` and `Debug` texts
    JoinErr {
        display: String,
        debug: String,
        is_panic: bool,
    },
    /// the buffer behind a `fmt::Formatter` handed to a user `fmt` impl
    Fmt {
        text: String,
        /// the impl went through `f.pad`, which honors the caller's width
        padded: bool,
    },
    /// left behind after a task is taken to await or a stdin pipe is closed
    Taken,
}

impl Native {
    pub fn type_name(&self) -> &'static str {
        match self {
            Native::Entry { .. } => "Entry",
            Native::Task(_) => "JoinHandle",
            Native::Future(_) => "Future",
            Native::HttpClient(_) | Native::BlockingHttpClient(_) => "Client",
            Native::ParseErr { .. } => "ParseError",
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
            Native::Fmt { .. } => "Formatter",
            Native::Taken => "Taken",
        }
    }

    pub fn as_read(&mut self) -> Option<&mut dyn Read> {
        match self {
            Native::File(r) => Some(r),
            Native::Reader(r) => Some(r),
            _ => None,
        }
    }

    /// Hands out the existing `BufReader` instead of wrapping a second one, that would eat bytes
    /// the next call expects.
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

/// Both format forms are captured from the real error, so `{:?}` prints the exact `Os { code: 2,
/// kind: NotFound, .. }` shape.
pub(super) fn io_error_value(e: &std::io::Error) -> Value {
    Native::IoErr {
        display: e.to_string(),
        debug: format!("{e:?}"),
        kind: format!("{:?}", e.kind()),
        code: e.raw_os_error(),
    }
    .wrap()
}
