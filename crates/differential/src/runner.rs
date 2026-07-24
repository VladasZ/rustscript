use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Classification {
    Match,
    /// Both ran to completion but printed different output.
    SemanticMismatch,
    /// Both panicked, but with different panic messages. A message the
    /// interpreter formats unlike the real compiler lands here.
    PanicMessageMismatch,
    /// The real binary panicked where the interpreter ran on. This is the
    /// overflow and narrowing-cast vein: the interpreter wraps or keeps an
    /// i64 where compiled Rust aborts.
    InterpreterMissingPanic,
    /// The interpreter panicked where the real binary finished cleanly.
    InterpreterSpuriousPanic,
    /// The interpreter reported a feature it does not implement. A gap to
    /// close in the interpreter, not a semantic bug.
    InterpreterUnsupported,
    /// The interpreter errored for a reason that is not a panic and not a
    /// declared gap.
    InterpreterCrash,
    InterpreterTimeout,
    NativeCrash,
    NativeTimeout,
    RustcRejected,
    RustcTimeout,
}

impl Classification {
    /// A real divergence worth saving and fixing. `Match` is agreement and
    /// `InterpreterUnsupported` is a known gap, so neither is hard.
    pub fn is_hard_failure(&self) -> bool {
        !matches!(self, Self::Match | Self::InterpreterUnsupported)
    }
}

/// A process that exits 101 is a panic, the code both the compiled binary and
/// the interpreter use for a runtime abort.
const PANIC_STATUS: i32 = 101;

/// How the native binary is compiled: the same way the default `cargo run`,
/// `build`, and `test` profiles do, with no optimization and overflow checks
/// on. That is the behavior a script author gets by default, so it is the
/// semantics RustScript targets, which means integer overflow must panic, not
/// wrap. It is also the only setting that lets the harness see an overflow
/// divergence at all, because with the checks off both sides wrap and agree.
/// Skipping optimization keeps each of the many compiles fast. Do not drop the
/// overflow flag.
const RUSTC_COMPILE_ARGS: [&str; 5] = ["--edition", "2024", "-C", "overflow-checks=yes", "-o"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub classification: Classification,
    pub compiler: ProcessOutput,
    pub native: ProcessOutput,
    pub interpreted: ProcessOutput,
}

impl RunResult {
    /// A short stable description of the concrete failure, so two different
    /// bugs with the same classification land in different buckets and the
    /// minimizer cannot drift from one bug to another. Digits are normalized
    /// because the same bug shows up with different values across seeds and
    /// across shrink steps.
    pub fn signature(&self) -> String {
        let raw = match &self.classification {
            Classification::PanicMessageMismatch => format!(
                "{} <> {}",
                panic_payload(&self.native.stderr),
                panic_payload(&self.interpreted.stderr)
            ),
            Classification::InterpreterMissingPanic => panic_payload(&self.native.stderr),
            Classification::InterpreterSpuriousPanic => panic_payload(&self.interpreted.stderr),
            Classification::SemanticMismatch => diff_site(&self.native, &self.interpreted),
            Classification::InterpreterCrash | Classification::InterpreterUnsupported => {
                first_meaningful_line(&self.interpreted.stderr)
            }
            _ => String::new(),
        };
        let mut signature = normalize_digits(&raw);
        signature.truncate(160);
        signature
    }

    /// The same bug for bucketing and reduction: classification and signature
    /// both agree.
    pub fn same_failure(&self, other: &Self) -> bool {
        self.classification == other.classification && self.signature() == other.signature()
    }
}

/// The label of the first output line that differs, which names the generated
/// case section that produced it. The values inside the line change with every
/// seed and every shrink step, so only the part before the first `:` is kept.
fn diff_site(native: &ProcessOutput, interpreted: &ProcessOutput) -> String {
    let streams = [
        (&native.stdout, &interpreted.stdout),
        (&native.stderr, &interpreted.stderr),
    ];
    for (native_stream, interpreted_stream) in streams {
        let mut native_lines = native_stream.lines();
        let mut interpreted_lines = interpreted_stream.lines();
        loop {
            match (native_lines.next(), interpreted_lines.next()) {
                (None, None) => break,
                (native_line, interpreted_line) => {
                    if native_line != interpreted_line {
                        let line = native_line.or(interpreted_line).unwrap_or_default();
                        return line.split(':').next().unwrap_or_default().to_string();
                    }
                }
            }
        }
    }
    String::new()
}

/// The first line of an error that carries information, used to group gaps and
/// crashes by the missing feature rather than by the exact values around it.
pub fn first_meaningful_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "Error:")
        .unwrap_or("unknown error")
        .to_string()
}

fn normalize_digits(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut in_number = false;
    for character in text.chars() {
        if character.is_ascii_digit() {
            if !in_number {
                normalized.push('N');
                in_number = true;
            }
        } else {
            in_number = false;
            normalized.push(character);
        }
    }
    normalized
}

pub struct Runner {
    interpreter: PathBuf,
    native_timeout: Duration,
    /// The interpreter gets four times the native budget. It is expected to be
    /// slower, and a shared budget would report near-boundary programs as
    /// spurious `InterpreterTimeout` findings. The compiler shares this larger
    /// budget because a cold rustc run is slow too.
    interpreted_timeout: Duration,
}

const INTERPRETED_TIMEOUT_FACTOR: u32 = 4;

impl Runner {
    pub fn build(workspace: &Path, timeout_ms: u64) -> Result<Self> {
        let interpreter = match std::env::var_os("RUSTSCRIPT_INTERPRETER") {
            Some(path) => PathBuf::from(path),
            None => {
                // A release interpreter runs campaigns several times faster
                // than the debug default. Point RUSTSCRIPT_INTERPRETER at a
                // debug build when its assertions are wanted.
                let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
                let status = Command::new(cargo)
                    .args(["build", "--release", "-p", "run-rs"])
                    .current_dir(workspace)
                    .status()
                    .context("failed to build RustScript")?;
                if !status.success() {
                    bail!("cargo build --release -p run-rs failed");
                }
                target_dir(workspace).join(executable_name("rust"))
            }
        };
        if !interpreter.is_file() {
            bail!("RustScript binary not found at {}", interpreter.display());
        }
        let interpreter = interpreter
            .canonicalize()
            .context("failed to resolve RustScript binary")?;
        let native_timeout = Duration::from_millis(timeout_ms);
        Ok(Self {
            interpreter,
            native_timeout,
            interpreted_timeout: native_timeout * INTERPRETED_TIMEOUT_FACTOR,
        })
    }

    pub fn run_source(&self, source: &str) -> Result<RunResult> {
        let directory = tempfile::Builder::new()
            .prefix("rustscript-differential-")
            .tempdir()?;
        let source_path = directory.path().join("case.rs");
        let binary_path = directory.path().join(executable_name("case"));
        fs::write(&source_path, source)?;

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiler = run_command(
            Command::new(rustc)
                .args(RUSTC_COMPILE_ARGS)
                .arg(&binary_path)
                .arg(&source_path)
                .current_dir(directory.path()),
            self.interpreted_timeout,
        )?;
        if compiler.timed_out {
            return Ok(incomplete(Classification::RustcTimeout, compiler));
        }
        if compiler.status != Some(0) {
            return Ok(incomplete(Classification::RustcRejected, compiler));
        }

        let native = run_command(
            Command::new(&binary_path).current_dir(directory.path()),
            self.native_timeout,
        )?;
        let interpreted = self.run_interpreted(&source_path, directory.path())?;
        let classification = classify(&native, &interpreted);
        Ok(RunResult {
            classification,
            compiler,
            native,
            interpreted,
        })
    }

    pub fn run_sources(&self, sources: &[String]) -> Result<Vec<RunResult>> {
        if sources.len() <= 1 {
            return sources
                .iter()
                .map(|source| self.run_source(source))
                .collect();
        }

        let directory = tempfile::Builder::new()
            .prefix("rustscript-differential-batch-")
            .tempdir()?;
        let bundle_path = directory.path().join("batch.rs");
        let binary_path = directory.path().join(executable_name("batch"));
        let source_paths = write_batch_sources(directory.path(), sources)?;
        fs::write(&bundle_path, render_native_batch(sources)?)?;

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiler = run_command(
            Command::new(rustc)
                .args(RUSTC_COMPILE_ARGS)
                .arg(&binary_path)
                .arg(&bundle_path)
                .current_dir(directory.path()),
            self.interpreted_timeout,
        )?;
        if compiler.timed_out || compiler.status != Some(0) {
            return sources
                .iter()
                .map(|source| self.run_source(source))
                .collect();
        }

        source_paths
            .iter()
            .enumerate()
            .map(|(index, source_path)| {
                let native = run_command(
                    Command::new(&binary_path)
                        .env("RUSTSCRIPT_DIFFERENTIAL_CASE", index.to_string())
                        .current_dir(directory.path()),
                    self.native_timeout,
                )?;
                let interpreted = self.run_interpreted(source_path, directory.path())?;
                let classification = classify(&native, &interpreted);
                Ok(RunResult {
                    classification,
                    compiler: compiler.clone(),
                    native,
                    interpreted,
                })
            })
            .collect()
    }

    fn run_interpreted(&self, source_path: &Path, directory: &Path) -> Result<ProcessOutput> {
        run_command(
            Command::new(&self.interpreter)
                .arg("run")
                .arg(source_path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1")
                .current_dir(directory),
            self.interpreted_timeout,
        )
    }
}

fn write_batch_sources(directory: &Path, sources: &[String]) -> Result<Vec<PathBuf>> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let path = directory.join(format!("case_{index}.rs"));
            fs::write(&path, source)?;
            Ok(path)
        })
        .collect()
}

fn render_native_batch(sources: &[String]) -> Result<String> {
    let mut bundle = String::new();
    for (index, source) in sources.iter().enumerate() {
        let module_source = source.replacen("fn main() {", "pub fn run() {", 1);
        if module_source == *source {
            bail!("generated source {index} has no main function");
        }
        bundle.push_str(&format!("mod case_{index} {{\n{module_source}\n}}\n\n"));
    }
    bundle.push_str(
        r#"fn main() {
    let index = std::env::var("RUSTSCRIPT_DIFFERENTIAL_CASE")
        .expect("missing case index")
        .parse::<usize>()
        .expect("invalid case index");
    match index {
"#,
    );
    for index in 0..sources.len() {
        bundle.push_str(&format!("        {index} => case_{index}::run(),\n"));
    }
    bundle.push_str(
        r#"        _ => panic!("case index out of range"),
    }
}
"#,
    );
    Ok(bundle)
}

fn target_dir(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        }
        None => workspace.join("target"),
    }
    .join("release")
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn incomplete(classification: Classification, compiler: ProcessOutput) -> RunResult {
    let empty = ProcessOutput {
        status: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    RunResult {
        classification,
        compiler,
        native: empty.clone(),
        interpreted: empty,
    }
}

fn classify(native: &ProcessOutput, interpreted: &ProcessOutput) -> Classification {
    if native.timed_out {
        return Classification::NativeTimeout;
    }
    if interpreted.timed_out {
        return Classification::InterpreterTimeout;
    }

    let native_panicked = native.status == Some(PANIC_STATUS);
    let interpreted_panicked = interpreted.status == Some(PANIC_STATUS);

    // A native exit that is neither success nor a panic is not something the
    // generator produces, so surface it rather than compare against it.
    if native.status != Some(0) && !native_panicked {
        return Classification::NativeCrash;
    }

    if native_panicked {
        return classify_native_panic(native, interpreted, interpreted_panicked);
    }

    // The real binary finished cleanly from here on.
    if interpreted_panicked {
        // A runtime-discovered gap aborts like a panic but names the missing
        // feature, `unknown method` or `unsupported constant`. That is a gap
        // to close, not a spurious panic.
        return if is_unsupported(&interpreted.stderr) {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterSpuriousPanic
        };
    }
    if interpreted.status != Some(0) {
        return if is_unsupported(&interpreted.stderr) {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterCrash
        };
    }
    if native.stdout == interpreted.stdout && native.stderr == interpreted.stderr {
        Classification::Match
    } else {
        Classification::SemanticMismatch
    }
}

fn classify_native_panic(
    native: &ProcessOutput,
    interpreted: &ProcessOutput,
    interpreted_panicked: bool,
) -> Classification {
    if !interpreted_panicked {
        // The interpreter ran past a point the compiled binary aborts at, the
        // overflow and narrowing-cast vein. An unsupported error is still a
        // gap even when it hides a missing panic.
        return if interpreted.status != Some(0) && is_unsupported(&interpreted.stderr) {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterMissingPanic
        };
    }
    // Both aborted. Output printed before the abort must still agree, and the
    // panic message the interpreter renders must match the real compiler.
    if native.stdout != interpreted.stdout {
        return Classification::SemanticMismatch;
    }
    if panic_payload(&native.stderr) == panic_payload(&interpreted.stderr) {
        Classification::Match
    } else if is_unsupported(&interpreted.stderr) {
        // The interpreter aborted on a missing feature where the native run
        // panicked for its own reason. Still a gap, not a message bug.
        Classification::InterpreterUnsupported
    } else {
        Classification::PanicMessageMismatch
    }
}

/// The interpreter marks missing-feature errors with a stable first-line
/// prefix. The loose substring check stays as a fallback for interpreter
/// binaries from before the prefix existed, reached via
/// `RUSTSCRIPT_INTERPRETER`.
fn is_unsupported(stderr: &str) -> bool {
    if stderr
        .lines()
        .any(|line| line.starts_with("rust unsupported:"))
    {
        return true;
    }
    let error = stderr.to_ascii_lowercase();
    error.contains("unsupported")
        || error.contains("not supported")
        || error.contains("not implemented by the interpreter")
        // Runtime bridge gaps abort mid-run with these wordings. Any program
        // that reaches the interpreter already passed rustc, so an unknown
        // name here is a missing bridge, not a generator bug.
        || error.contains("unknown method")
        || error.contains("unknown function")
}

/// The message a panic carries, without the location line or the backtrace
/// note. The compiled binary prints `panicked at FILE:LINE:COL:` and a
/// `note: run with RUST_BACKTRACE` line the interpreter never emits, so those
/// are dropped before the payloads are compared.
fn panic_payload(stderr: &str) -> String {
    let mut lines = stderr.lines();
    for line in lines.by_ref() {
        if line.contains("panicked at") {
            break;
        }
    }
    lines
        .map(str::trim)
        .take_while(|line| !is_backtrace_line(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A line that belongs to a backtrace or the backtrace hint, not the panic
/// message. The compiled binary prints `note: run with RUST_BACKTRACE`; the
/// interpreter prints its own script frames as `at <function> (<file>:<line>)`
/// with a `... N more frames` tail. Neither is part of the message compared.
fn is_backtrace_line(line: &str) -> bool {
    line.starts_with("note:")
        || line.starts_with("at ")
        || (line.starts_with("...") && line.ends_with("more frames"))
}

fn run_command(command: &mut Command, timeout: Duration) -> Result<ProcessOutput> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {:?}", command.get_program()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("child stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("child stderr was not captured"))?;
    let stdout_reader = spawn(move || read_pipe(stdout));
    let stderr_reader = spawn(move || read_pipe(stderr));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            child.kill().context("failed to kill timed out process")?;
            break (child.wait()?, true);
        }
        sleep(Duration::from_millis(5));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("child stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("child stderr reader panicked"))??;
    Ok(ProcessOutput {
        status: status.code(),
        stdout,
        stderr,
        timed_out,
    })
}

fn read_pipe(mut pipe: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(status: i32, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            status: Some(status),
            stdout: String::new(),
            stderr: stderr.to_string(),
            timed_out: false,
        }
    }

    #[test]
    fn unsupported_errors_are_gaps() {
        assert_eq!(
            classify(&output(0, ""), &output(1, "unsupported item: macro")),
            Classification::InterpreterUnsupported
        );
    }

    #[test]
    fn different_output_is_a_semantic_failure() {
        let native = ProcessOutput {
            stdout: "one".to_string(),
            ..output(0, "")
        };
        let interpreted = ProcessOutput {
            stdout: "two".to_string(),
            ..output(0, "")
        };
        assert_eq!(
            classify(&native, &interpreted),
            Classification::SemanticMismatch
        );
    }

    fn panic(payload: &str) -> ProcessOutput {
        ProcessOutput {
            status: Some(PANIC_STATUS),
            stdout: String::new(),
            stderr: format!(
                "thread 'main' panicked at case.rs:1:1:\n{payload}\nnote: run with `RUST_BACKTRACE=1`\n"
            ),
            timed_out: false,
        }
    }

    #[test]
    fn matching_panics_agree_despite_location_and_backtrace_noise() {
        assert_eq!(
            classify(
                &panic("attempt to add with overflow"),
                &panic("attempt to add with overflow")
            ),
            Classification::Match
        );
    }

    #[test]
    fn interpreter_script_backtrace_is_not_part_of_the_message() {
        // The interpreter appends `at <frame>` lines the compiled binary never
        // prints; the same overflow must still read as agreement.
        let native = panic("attempt to multiply with overflow");
        let interpreted = ProcessOutput {
            status: Some(PANIC_STATUS),
            stdout: String::new(),
            stderr: "thread 'main' panicked at case_0.rs:82:\nattempt to multiply with overflow\n  at main (case_0.rs:82)\n".to_string(),
            timed_out: false,
        };
        assert_eq!(classify(&native, &interpreted), Classification::Match);
    }

    #[test]
    fn interpreter_running_past_a_real_panic_is_a_finding() {
        let native = panic("attempt to add with overflow");
        let interpreted = ProcessOutput {
            stdout: "9223372036854775808".to_string(),
            ..output(0, "")
        };
        assert_eq!(
            classify(&native, &interpreted),
            Classification::InterpreterMissingPanic
        );
    }

    #[test]
    fn interpreter_panicking_alone_is_a_finding() {
        assert_eq!(
            classify(&output(0, ""), &panic("attempt to divide by zero")),
            Classification::InterpreterSpuriousPanic
        );
    }

    #[test]
    fn a_runtime_gap_panic_is_a_gap() {
        assert_eq!(
            classify(
                &output(0, ""),
                &panic("unknown method `product` on Iterator")
            ),
            Classification::InterpreterUnsupported
        );
        assert_eq!(
            classify(
                &panic("attempt to add with overflow"),
                &panic("unsupported constant `f64::LOG2_10`")
            ),
            Classification::InterpreterUnsupported
        );
    }

    #[test]
    fn differing_panic_messages_are_a_finding() {
        assert_eq!(
            classify(
                &panic("range end index 5 out of range for slice of length 1"),
                &panic("slice 0..5 out of bounds (len 1)")
            ),
            Classification::PanicMessageMismatch
        );
    }

    #[test]
    fn a_gap_that_hides_a_missing_panic_stays_a_gap() {
        let native = panic("attempt to add with overflow");
        let interpreted = output(1, "unsupported item: macro");
        assert_eq!(
            classify(&native, &interpreted),
            Classification::InterpreterUnsupported
        );
    }

    #[test]
    fn large_captured_output_does_not_block() -> Result<()> {
        let output = run_command(
            Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "runner::tests::large_output_helper",
                    "--nocapture",
                ])
                .env("RUSTSCRIPT_TEST_LARGE_OUTPUT", "1"),
            Duration::from_secs(10),
        )?;

        assert!(!output.timed_out);
        assert_eq!(output.status, Some(0));
        assert!(output.stderr.len() >= 1024 * 1024);
        Ok(())
    }

    #[test]
    fn large_output_helper() {
        if std::env::var_os("RUSTSCRIPT_TEST_LARGE_OUTPUT").is_some() {
            eprint!("{}", "x".repeat(1024 * 1024));
        }
    }
}
