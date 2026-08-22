//! Every run gets a hard timeout, so a deadlock fails the test naming the
//! script instead of hanging CI. The struct equality deadlock slipped through
//! for exactly that reason.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Generous next to the slowest example, tight next to a real deadlock.
const TIMEOUT: Duration = Duration::from_mins(2);

/// A run that outlives the timeout is killed and fails the test with the
/// label.
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

/// Own thread per pipe, so the child never blocks on a full pipe while main
/// only polls `try_wait`.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            // A read error means the child died mid write, usually killed by
            // the timeout. The bytes read so far are the best diagnostic.
            if let Err(e) = pipe.read_to_end(&mut buf) {
                eprintln!("pipe read failed: {e}");
            }
        }
        buf
    })
}
