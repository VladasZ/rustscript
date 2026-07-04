//! Benchmark orchestrator. For every case it first proves all four languages
//! print byte identical stdout, then measures two tracks. The wall-clock track
//! uses hyperfine, startup included, what you feel when you run the script. The
//! compute track reads each run's self timed COMPUTE_NS from stderr, startup
//! excluded, the language at the actual work. rustscript is always measured warm
//! with the check gate skipped. The gate cost is measured once on its own.
//!
//! Usage: cargo run --release --bin bench [-- --quick]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use rustscript_bench::{CaseResult, CompStat, Gate, LANGS, Report, WallStat};

/// One benchmark case and how to invoke each language.
struct Case {
    name: &'static str,
    kind: &'static str,
    /// Data file under the case dir passed as the single argument, or "".
    data: &'static str,
}

const CASES: &[Case] = &[
    Case { name: "hello", kind: "startup", data: "" },
    Case { name: "fib", kind: "compute", data: "" },
    Case { name: "sieve", kind: "compute", data: "" },
    Case { name: "mandelbrot", kind: "compute", data: "" },
    Case { name: "collatz", kind: "compute", data: "" },
    Case { name: "word_count", kind: "compute", data: "data.txt" },
    Case { name: "json", kind: "compute", data: "data.json" },
];

fn main() -> Result<()> {
    let quick = std::env::args().any(|a| a == "--quick");
    let no_gate = std::env::args().any(|a| a == "--no-gate");
    let wall_runs = if quick { 5 } else { 10 };
    let comp_samples = if quick { 6 } else { 15 };

    let root = workspace_root()?;
    let results_dir = root.join("bench/results");
    std::fs::create_dir_all(&results_dir)?;

    ensure_tool("hyperfine")?;
    ensure_tool("bun")?;
    ensure_tool("python3")?;

    println!("building native example binaries ...");
    let build = Command::new(env!("CARGO"))
        .args(["build", "--release", "--examples", "-p", "rustscript-bench"])
        .current_dir(&root)
        .status()?;
    if !build.success() {
        bail!("cargo build --examples failed");
    }

    let mut cases = Vec::new();
    for c in CASES {
        println!("\n== {} ==", c.name);
        let inv: Vec<Invocation> = LANGS.iter().map(|l| invocation(&root, c, l)).collect();

        gate_check(c, &inv)?;

        let mut wall = Vec::new();
        for iv in &inv {
            let s = hyperfine(iv, wall_runs, &results_dir)?;
            println!("  wall   {:<11} {:>8.1} ms", iv.lang, s.mean * 1e3);
            wall.push(s);
        }

        let mut compute = Vec::new();
        if c.kind != "startup" {
            for iv in &inv {
                let s = compute_track(iv, comp_samples)?;
                println!("  compute{:<11} {:>8.1} ms", iv.lang, s.min * 1e3);
                compute.push(s);
            }
        }

        cases.push(CaseResult {
            name: c.name.to_string(),
            kind: c.kind.to_string(),
            wall,
            compute,
        });
    }

    println!("\n== check gate cost ==");
    let gate = if no_gate {
        let prev: Report =
            serde_json::from_str(&std::fs::read_to_string(results_dir.join("results.json"))?)
                .context("--no-gate needs a previous results.json")?;
        println!("  reused from previous run");
        prev.gate
    } else {
        gate_cost(&root, &results_dir)?
    };
    println!("  cold {:>7.2} s   warm {:>7.1} ms", gate.cold_mean, gate.warm_mean * 1e3);

    let report = Report { cases, gate };
    let out = results_dir.join("results.json");
    std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
    println!("\nwrote {}", out.display());
    println!("now run: cargo run --release --bin chart");
    Ok(())
}

/// How to launch one language for one case.
struct Invocation {
    lang: String,
    program: String,
    args: Vec<String>,
    skip_check: bool,
}

fn invocation(root: &Path, c: &Case, lang: &str) -> Invocation {
    let case_dir = root.join("bench/cases").join(c.name);
    let arg: Vec<String> = if c.data.is_empty() {
        vec![]
    } else {
        vec![case_dir.join(c.data).to_string_lossy().into_owned()]
    };
    match lang {
        "native" => {
            let mut args = vec![];
            args.extend(arg);
            Invocation {
                lang: lang.into(),
                program: root
                    .join("target/release/examples")
                    .join(c.name)
                    .to_string_lossy()
                    .into_owned(),
                args,
                skip_check: false,
            }
        }
        "rustscript" => {
            let mut args = vec!["run".to_string(), case_dir.join("case.rs").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation { lang: lang.into(), program: "rust".into(), args, skip_check: true }
        }
        "bun" => {
            let mut args = vec!["run".to_string(), case_dir.join("case.ts").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation { lang: lang.into(), program: "bun".into(), args, skip_check: false }
        }
        "python" => {
            let mut args = vec![case_dir.join("case.py").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation { lang: lang.into(), program: "python3".into(), args, skip_check: false }
        }
        _ => unreachable!(),
    }
}

impl Invocation {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if self.skip_check {
            cmd.env("RUSTSCRIPT_SKIP_CHECK", "1");
        }
        cmd
    }

    /// The command as a single line for hyperfine `--shell=none`. Paths in this
    /// repo carry no spaces, so plain join is safe and avoids shell noise on the
    /// sub millisecond startup measurements.
    fn cmdline(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }
}

/// Prove all four languages produce identical stdout for a case.
fn gate_check(c: &Case, inv: &[Invocation]) -> Result<()> {
    let mut baseline: Option<(String, Vec<u8>)> = None;
    for iv in inv {
        let out = iv.command().output().with_context(|| format!("running {} {}", iv.lang, c.name))?;
        if !out.status.success() {
            bail!("{} {} exited with error:\n{}", iv.lang, c.name, String::from_utf8_lossy(&out.stderr));
        }
        match &baseline {
            None => baseline = Some((iv.lang.clone(), out.stdout)),
            Some((blang, bout)) => {
                if *bout != out.stdout {
                    bail!(
                        "output mismatch for {}: {} vs {}\n-- {} --\n{}\n-- {} --\n{}",
                        c.name, blang, iv.lang, blang,
                        String::from_utf8_lossy(bout), iv.lang,
                        String::from_utf8_lossy(&out.stdout),
                    );
                }
            }
        }
    }
    println!("  gate   ok, all four match");
    Ok(())
}

#[derive(serde::Deserialize)]
struct HfFile {
    results: Vec<HfResult>,
}

#[derive(serde::Deserialize)]
struct HfResult {
    mean: f64,
    stddev: Option<f64>,
    median: f64,
    min: f64,
}

/// Wall-clock track via hyperfine, warmup priming the cache so every measured
/// run is warm and gate free.
fn hyperfine(iv: &Invocation, runs: u32, dir: &Path) -> Result<WallStat> {
    let json = dir.join(format!("hf_{}.json", iv.lang));
    let mut cmd = Command::new("hyperfine");
    cmd.args([
        "--warmup", "3",
        "--runs", &runs.to_string(),
        "-N",
        "--export-json", &json.to_string_lossy(),
    ])
    .args(["--command-name", &iv.lang])
    .arg(iv.cmdline())
    .stdout(std::process::Stdio::null());
    // Passed through to the child so rustscript stays warm and gate free. The
    // other languages simply ignore it.
    if iv.skip_check {
        cmd.env("RUSTSCRIPT_SKIP_CHECK", "1");
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("hyperfine failed for {}", iv.lang);
    }
    let parsed: HfFile = serde_json::from_str(&std::fs::read_to_string(&json)?)?;
    let r = &parsed.results[0];
    Ok(WallStat {
        lang: iv.lang.clone(),
        mean: r.mean,
        stddev: r.stddev.unwrap_or(0.0),
        min: r.min,
        median: r.median,
    })
}

/// Compute track. Run the case a number of times, read the self timed
/// COMPUTE_NS from stderr each time, keep the min and median.
fn compute_track(iv: &Invocation, samples: u32) -> Result<CompStat> {
    let mut secs = Vec::new();
    for _ in 0..samples {
        let out = iv.command().output()?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let ns = stderr
            .lines()
            .find_map(|l| {
                // Bun colors `console.error` even when piped, so the marker can
                // arrive wrapped in ANSI escapes. Take the digits after it.
                let start = l.find("COMPUTE_NS")? + "COMPUTE_NS".len();
                let digits: String = l[start..]
                    .chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                digits.parse::<f64>().ok()
            })
            .with_context(|| format!("no COMPUTE_NS from {}", iv.lang))?;
        secs.push(ns / 1e9);
    }
    secs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = secs[0];
    let median = secs[secs.len() / 2];
    Ok(CompStat { lang: iv.lang.clone(), min, median })
}

/// Measure the one-time gate. Cold clears the cache before each run so the full
/// `cargo check` runs. Warm runs the same script with the cache already primed.
fn gate_cost(root: &Path, dir: &Path) -> Result<Gate> {
    let script = root.join("bench/cases/hello/case.rs");
    let script = script.to_string_lossy().into_owned();
    let run = format!("rust run {script}");

    // Warm: prime once, then measure with cache hits.
    let _ = Command::new("rust").args(["run", &script]).status();
    let warm_json = dir.join("hf_gate_warm.json");
    Command::new("hyperfine")
        .args(["--warmup", "2", "--runs", "10", "-N", "--export-json"])
        .arg(&warm_json)
        .arg(&run)
        .stdout(std::process::Stdio::null())
        .status()?;
    let warm: HfFile = serde_json::from_str(&std::fs::read_to_string(&warm_json)?)?;

    // Cold: clear the whole cache before every run so each pays the full check.
    let cold_json = dir.join("hf_gate_cold.json");
    Command::new("hyperfine")
        .args(["--runs", "3", "-N", "--prepare", "rust clean", "--export-json"])
        .arg(&cold_json)
        .arg(&run)
        .stdout(std::process::Stdio::null())
        .status()?;
    let cold: HfFile = serde_json::from_str(&std::fs::read_to_string(&cold_json)?)?;

    Ok(Gate { cold_mean: cold.results[0].mean, warm_mean: warm.results[0].mean })
}

fn workspace_root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("no parent")?.to_path_buf())
}

fn ensure_tool(name: &str) -> Result<()> {
    let ok = Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!("required tool `{name}` not found on PATH");
    }
    Ok(())
}
