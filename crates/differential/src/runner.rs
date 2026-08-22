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
    /// both ran to completion with different output
    SemanticMismatch,
    /// both panicked with different messages
    PanicMessageMismatch,
    /// the real binary panicked where the interpreter ran on, the overflow and narrowing cast vein
    InterpreterMissingPanic,
    /// the interpreter panicked where the real binary finished cleanly
    InterpreterSpuriousPanic,
    /// a declared gap in the interpreter, not a semantic bug
    InterpreterUnsupported,
    /// neither a panic nor a declared gap
    InterpreterCrash,
    InterpreterTimeout,
    NativeCrash,
    NativeTimeout,
    /// 2 runs of the native binary disagreed, so a grammar hole let nondeterminism through.
    /// Counted, never reported as a bug.
    NativeNondeterministic,
    RustcRejected,
    RustcTimeout,
}

impl Classification {
    /// A real divergence. A gap or a nondeterministic case is not one.
    pub fn is_hard_failure(&self) -> bool {
        !matches!(
            self,
            Self::Match | Self::InterpreterUnsupported | Self::NativeNondeterministic
        )
    }
}

/// Exit 101 is a panic on both sides.
const PANIC_STATUS: i32 = 101;

/// Debug profile, no optimization and overflow checks on. That is what `RustScript` targets, and with
/// the checks off both sides wrap and agree. Don't drop the overflow flag.
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
    /// A short stable description of the concrete failure, so 2 bugs with the same classification land
    /// in different buckets. Digits are normalized because values change across seeds and shrink steps.
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
            // the reason, not the location header, otherwise gaps all collapse into 1 bucket
            Classification::InterpreterCrash | Classification::InterpreterUnsupported => {
                gap_reason(&self.interpreted.stderr)
            }
            _ => String::new(),
        };
        let mut signature = normalize_digits(&raw);
        signature.truncate(160);
        signature
    }

    /// The same bug for bucketing and reduction.
    pub fn same_failure(&self, other: &Self) -> bool {
        self.classification == other.classification && self.signature() == other.signature()
    }
}

/// The label of the first differing line. The values change with every seed and shrink step, so
/// only the part before the first `:` is kept.
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

/// The reason of a gap or crash. An interpreter panic carries it on the line after the `panicked
/// at` header, a plain `rust error:` on the first line.
pub fn gap_reason(stderr: &str) -> String {
    if stderr.contains("panicked at") {
        let payload = panic_payload(stderr);
        if let Some(reason) = payload.lines().next()
            && !reason.is_empty()
        {
            return reason.to_string();
        }
    }
    first_meaningful_line(stderr)
}

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
    /// The interpreter gets 4 times the native budget, or near boundary programs report spurious
    /// timeouts. A cold `rustc` shares it.
    interpreted_timeout: Duration,
}

const INTERPRETED_TIMEOUT_FACTOR: u32 = 4;

impl Runner {
    /// The `rust supported` listing.
    pub fn supported_listing(&self) -> Result<String> {
        let output = Command::new(&self.interpreter).arg("supported").output()?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn build(workspace: &Path, timeout_ms: u64) -> Result<Self> {
        let interpreter = if let Some(path) = std::env::var_os("RUSTSCRIPT_INTERPRETER") {
            PathBuf::from(path)
        } else {
            // a release interpreter is several times faster, point `RUSTSCRIPT_INTERPRETER` at a
            // debug build for its assertions
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
        // the reference runs twice, if it disagrees with itself the case proves nothing about the
        // interpreter
        let rerun = run_command(
            Command::new(&binary_path).current_dir(directory.path()),
            self.native_timeout,
        )?;
        if !same_native_run(&native, &rerun) {
            return Ok(incomplete(Classification::NativeNondeterministic, compiler));
        }
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
                let rerun = run_command(
                    Command::new(&binary_path)
                        .env("RUSTSCRIPT_DIFFERENTIAL_CASE", index.to_string())
                        .current_dir(directory.path()),
                    self.native_timeout,
                )?;
                if !same_native_run(&native, &rerun) {
                    return Ok(incomplete(
                        Classification::NativeNondeterministic,
                        compiler.clone(),
                    ));
                }
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

    // the generator never produces a native exit that is neither success nor a panic, so report
    // it instead of comparing
    if native.status != Some(0) && !native_panicked {
        return Classification::NativeCrash;
    }

    // a `rust error:` with nothing printed is a load rejection, classify by that or 1 load bug
    // scatters across unrelated buckets
    if interpreted.status == Some(1)
        && interpreted.stdout.is_empty()
        && interpreted
            .stderr
            .lines()
            .next()
            .is_some_and(|line| line.starts_with("rust error:"))
    {
        return if is_unsupported(&interpreted.stderr) {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterCrash
        };
    }

    if native_panicked {
        return classify_native_panic(native, interpreted, interpreted_panicked);
    }

    if interpreted_panicked {
        // a runtime gap aborts like a panic but names the missing feature
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
        // an unsupported error is still a gap even when it hides a missing panic
        return if interpreted.status != Some(0) && is_unsupported(&interpreted.stderr) {
            Classification::InterpreterUnsupported
        } else {
            Classification::InterpreterMissingPanic
        };
    }
    // Both aborted. Check the gap first, it stops the interpreter earlier than the native panic,
    // comparing stdout first reports a false `SemanticMismatch`.
    if is_unsupported(&interpreted.stderr) {
        return Classification::InterpreterUnsupported;
    }
    // output before the abort and the panic message must both agree
    if native.stdout != interpreted.stdout {
        return Classification::SemanticMismatch;
    }
    if panic_payload(&native.stderr) == panic_payload(&interpreted.stderr) {
        Classification::Match
    } else {
        Classification::PanicMessageMismatch
    }
}

/// The loose substring check is a fallback for interpreter binaries from before the prefix existed.
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
        // the program already passed `rustc`, so an unknown name is a missing bridge, not a
        // generator bug
        || error.contains("unknown method")
        || error.contains("unknown function")
}

/// The panic header carries the thread id, which changes per process, so stderr is compared
/// through `panic_payload`.
fn same_native_run(first: &ProcessOutput, second: &ProcessOutput) -> bool {
    first.status == second.status
        && first.timed_out == second.timed_out
        && first.stdout == second.stdout
        && panic_payload(&first.stderr) == panic_payload(&second.stderr)
}

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

/// The compiled binary prints `note: run with RUST_BACKTRACE`, the interpreter prints `at
/// <function> (<file>:<line>)` frames. Neither is part of the compared message.
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
        .with_context(|| format!("failed to launch {}", command.get_program().display()))?;
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

    /// Keying on the first stderr line collapses every gap into 1 bucket.
    #[test]
    fn gaps_bucket_by_reason_not_location() {
        let one = "thread 'main' panicked at case_3.rs:12:\nunknown method `ilog2` on a number\n  at main (case_3.rs:12)\n";
        let two = "thread 'main' panicked at case_3.rs:12:\nunknown method `leading_ones` on a number\n  at main (case_3.rs:12)\n";
        assert_eq!(gap_reason(one), "unknown method `ilog2` on a number");
        assert_eq!(gap_reason(two), "unknown method `leading_ones` on a number");
        assert_eq!(
            gap_reason("rust unsupported: macro `todo`"),
            "rust unsupported: macro `todo`"
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
        // the interpreter's `at <frame>` lines must not break agreement
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
