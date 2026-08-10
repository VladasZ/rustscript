//! Shared runner for the suites that execute scripts and compiled examples.
//! Every run gets a hard timeout, so an interpreter deadlock fails the test
//! naming the script instead of hanging the whole cargo test run. The struct
//! equality deadlock that froze every `PathBuf` comparison slipped through
//! these suites for exactly that reason: nothing here compared structs, and
//! a hang would have stalled CI rather than failing it.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Generous next to the slowest example, tight next to a real deadlock.
const TIMEOUT: Duration = Duration::from_mins(2);

/// Run to completion and return success, captured stdout, and captured
/// stderr. A run that outlives the timeout is killed and fails the test with
/// the label.
pub fn run(cmd: &mut Command, label: &str) -> (bool, Vec<u8>, Vec<u8>) {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("cannot spawn `{label}`: {e}"));
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return (
                    status.success(),
                    stdout.join().expect("stdout reader"),
                    stderr.join().expect("stderr reader"),
                );
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    if let Err(e) = child.kill() {
                        eprintln!("cannot kill timed out `{label}`: {e}");
                    }
                    panic!(
                        "`{label}` still running after {} seconds, likely an interpreter hang or deadlock",
                        TIMEOUT.as_secs()
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("cannot wait for `{label}`: {e}"),
        }
    }
}

/// Read a pipe to the end on its own thread, so the child never blocks on a
/// full pipe while the main thread only polls `try_wait`.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            // A read error here means the child died mid write, usually
            // because the timeout killed it. The bytes read so far are still
            // the best diagnostic there is.
            if let Err(e) = pipe.read_to_end(&mut buf) {
                eprintln!("pipe read failed: {e}");
            }
        }
        buf
    })
}
