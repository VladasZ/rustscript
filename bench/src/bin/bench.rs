//! Benchmark orchestrator. For every case and tier it first proves all four
//! languages print byte identical stdout, then measures three tracks. The
//! wall-clock track uses hyperfine, startup included, what you feel when you
//! run the script. The compute track reads each run's self timed COMPUTE_NS
//! from stderr, startup excluded, the language at the actual work. The memory
//! track records peak RSS via /usr/bin/time. rustscript is always measured warm
//! with the check gate skipped, and node gets its own compile cache for the
//! same reason. The gate cost is measured once on its own.
//!
//! Every compute case runs at two sizes. The base tier is small, where startup
//! dominates the wall clock. The big tier is 10x, where the work dominates and
//! JIT warmup has amortized.
//!
//! Usage: cargo run --release --bin bench [-- --quick] [-- --no-gate]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rustscript_bench::{CaseResult, CompStat, Gate, LANGS, MemStat, Meta, Report, WallStat};

/// How one case gets its work size for the two tiers.
enum Input {
    /// No argument, startup cases.
    None,
    /// A numeric size passed as the single argument.
    Size { base: u64, big: u64 },
    /// A data file under the case dir passed as the single argument.
    Data { base: &'static str, big: &'static str },
}

/// One benchmark case and how to invoke each language.
struct Case {
    name: &'static str,
    kind: &'static str,
    input: Input,
}

const CASES: &[Case] = &[
    Case { name: "hello", kind: "startup", input: Input::None },
    Case { name: "big_script", kind: "startup", input: Input::None },
    Case { name: "fib", kind: "compute", input: Input::Size { base: 27, big: 32 } },
    Case { name: "sieve", kind: "compute", input: Input::Size { base: 250_000, big: 2_500_000 } },
    Case { name: "mandelbrot", kind: "compute", input: Input::Size { base: 140, big: 440 } },
    Case { name: "collatz", kind: "compute", input: Input::Size { base: 10_000, big: 100_000 } },
    Case { name: "binary_trees", kind: "compute", input: Input::Size { base: 11, big: 14 } },
    Case { name: "string_builder", kind: "compute", input: Input::Size { base: 200_000, big: 2_000_000 } },
    Case { name: "higher_order", kind: "compute", input: Input::Size { base: 100_000, big: 1_000_000 } },
    Case { name: "sort", kind: "compute", input: Input::Size { base: 50_000, big: 500_000 } },
    Case { name: "hashmap_int", kind: "compute", input: Input::Size { base: 150_000, big: 1_500_000 } },
    Case { name: "nbody", kind: "compute", input: Input::Size { base: 8_000, big: 80_000 } },
    Case { name: "json_serialize", kind: "compute", input: Input::Size { base: 100_000, big: 1_000_000 } },
    Case { name: "stdout_lines", kind: "compute", input: Input::Size { base: 20_000, big: 200_000 } },
    Case {
        name: "word_count",
        kind: "compute",
        input: Input::Data { base: "data.txt", big: "data_big.txt" },
    },
    Case {
        name: "json",
        kind: "compute",
        input: Input::Data { base: "data.json", big: "data_big.json" },
    },
    Case {
        name: "regex",
        kind: "compute",
        input: Input::Data { base: "../word_count/data.txt", big: "../word_count/data_big.txt" },
    },
];

fn main() -> Result<()> {
    let quick = std::env::args().any(|a| a == "--quick");
    let no_gate = std::env::args().any(|a| a == "--no-gate");

    let root = workspace_root()?;
    let results_dir = root.join("bench/results");
    std::fs::create_dir_all(&results_dir)?;
    let node_cache = results_dir.join("node_cache");
    std::fs::create_dir_all(&node_cache)?;

    ensure_tool("hyperfine")?;
    ensure_tool("node")?;
    ensure_tool("python3")?;

    println!("building native example binaries ...");
    let build = Command::new(env!("CARGO"))
        .args(["build", "--release", "--examples", "-p", "rustscript-bench"])
        .current_dir(&root)
        .status()?;
    if !build.success() {
        bail!("cargo build --examples failed");
    }

    ensure_big_data(&root)?;

    let mut cases = Vec::new();
    for c in CASES {
        for tier in ["base", "big"] {
            if tier == "big" && matches!(c.input, Input::None) {
                continue;
            }
            // The big tier runs 10x the work, so it gets fewer samples.
            let (wall_runs, comp_samples) = match (tier, quick) {
                ("base", false) => (10, 15),
                ("base", true) => (5, 6),
                ("big", false) => (5, 5),
                ("big", true) => (3, 3),
                _ => unreachable!(),
            };

            println!("\n== {} {} ==", c.name, tier);
            let inv: Vec<Invocation> =
                LANGS.iter().map(|l| invocation(&root, c, l, tier, &node_cache)).collect();

            gate_check(c, &inv)?;

            let mut wall = Vec::new();
            for iv in &inv {
                let s = hyperfine(iv, wall_runs, &results_dir)?;
                println!("  wall   {:<11} {:>8.1} ms", iv.lang, s.mean * 1e3);
                wall.push(s);
            }

            let (compute, mem) = if c.kind == "startup" {
                (Vec::new(), rss_track(&inv, 3)?)
            } else {
                compute_track(&inv, comp_samples)?
            };
            for s in &compute {
                println!("  compute{:<11} {:>8.1} ms", s.lang, s.min * 1e3);
            }
            for m in &mem {
                println!("  rss    {:<11} {:>8.1} MB", m.lang, m.rss_bytes as f64 / 1e6);
            }

            cases.push(CaseResult {
                name: c.name.to_string(),
                kind: c.kind.to_string(),
                tier: tier.to_string(),
                wall,
                compute,
                mem,
            });
        }
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

    let report = Report { meta: gather_meta()?, cases, gate };
    let out = results_dir.join("results.json");
    std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
    println!("\nwrote {}", out.display());
    println!("now run: cargo run --release --bin chart");
    Ok(())
}

/// The 10x data files are gitignored for size. Regenerate them when missing.
fn ensure_big_data(root: &Path) -> Result<()> {
    let wc = root.join("bench/cases/word_count/data_big.txt");
    let js = root.join("bench/cases/json/data_big.json");
    if wc.exists() && js.exists() {
        return Ok(());
    }
    println!("generating big tier data files ...");
    let status = Command::new(env!("CARGO"))
        .args(["run", "--release", "--bin", "gendata", "--", "--big"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("gendata --big failed");
    }
    Ok(())
}

/// Tool versions and hardware, stored in the report so runs stay comparable.
fn gather_meta() -> Result<Meta> {
    let line = |program: &str, args: &[&str]| -> String {
        Command::new(program)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    Ok(Meta {
        date_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        rustc: line("rustc", &["--version"]),
        node: line("node", &["--version"]),
        python: line("python3", &["--version"]),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu: if cfg!(target_os = "macos") {
            line("sysctl", &["-n", "machdep.cpu.brand_string"])
        } else {
            line("uname", &["-p"])
        },
    })
}

/// How to launch one language for one case at one tier.
struct Invocation {
    lang: String,
    program: String,
    args: Vec<String>,
    /// Extra environment for both direct runs and hyperfine, which passes its
    /// own environment through to the measured child.
    env: Vec<(String, String)>,
}

fn invocation(root: &Path, c: &Case, lang: &str, tier: &str, node_cache: &Path) -> Invocation {
    let case_dir = root.join("bench/cases").join(c.name);
    let arg: Vec<String> = match &c.input {
        Input::None => vec![],
        Input::Size { base, big } => {
            vec![if tier == "big" { big.to_string() } else { base.to_string() }]
        }
        Input::Data { base, big } => {
            let file = if tier == "big" { big } else { base };
            vec![case_dir.join(file).to_string_lossy().into_owned()]
        }
    };
    match lang {
        "native" => Invocation {
            lang: lang.into(),
            program: root
                .join("target/release/examples")
                .join(c.name)
                .to_string_lossy()
                .into_owned(),
            args: arg,
            env: vec![],
        },
        "rustscript" => {
            let mut args =
                vec!["run".to_string(), case_dir.join("case.rs").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation {
                lang: lang.into(),
                program: "rust".into(),
                args,
                // Warm and gate free, the one-time check cost is measured
                // separately as the gate.
                env: vec![("RUSTSCRIPT_SKIP_CHECK".into(), "1".into())],
            }
        }
        "node" => {
            let mut args = vec![case_dir.join("case.ts").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation {
                lang: lang.into(),
                program: "node".into(),
                args,
                // The V8 compile cache, so node does not re-strip and
                // re-compile the script on every run while rustscript runs
                // from its warm cache.
                env: vec![(
                    "NODE_COMPILE_CACHE".into(),
                    node_cache.to_string_lossy().into_owned(),
                )],
            }
        }
        "python" => {
            let mut args = vec![case_dir.join("case.py").to_string_lossy().into_owned()];
            args.extend(arg);
            Invocation { lang: lang.into(), program: "python3".into(), args, env: vec![] }
        }
        _ => unreachable!(),
    }
}

impl Invocation {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }

    /// The same invocation wrapped in /usr/bin/time so stderr carries the peak
    /// RSS next to the case's own COMPUTE_NS line.
    fn timed_command(&self) -> Command {
        let mut cmd = Command::new("/usr/bin/time");
        cmd.arg(if cfg!(target_os = "macos") { "-l" } else { "-v" });
        cmd.arg(&self.program);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
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
    // Passed through to the measured child. Other languages ignore foreign
    // variables.
    for (k, v) in &iv.env {
        cmd.env(k, v);
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

/// Compute and memory track. Samples are interleaved round robin across
/// languages so slow thermal drift spreads evenly instead of biasing whichever
/// language runs last. Each run goes through /usr/bin/time, whose stderr adds
/// peak RSS next to the case's self timed COMPUTE_NS. The wrapper only affects
/// wall time, never the self timed value.
fn compute_track(inv: &[Invocation], samples: u32) -> Result<(Vec<CompStat>, Vec<MemStat>)> {
    let mut secs: Vec<Vec<f64>> = vec![Vec::new(); inv.len()];
    let mut rss: Vec<u64> = vec![0; inv.len()];
    for _ in 0..samples {
        for (i, iv) in inv.iter().enumerate() {
            let out = iv.timed_command().output()?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            let ns = parse_compute_ns(&stderr)
                .with_context(|| format!("no COMPUTE_NS from {}", iv.lang))?;
            secs[i].push(ns / 1e9);
            if let Some(b) = parse_rss_bytes(&stderr) {
                rss[i] = rss[i].max(b);
            }
        }
    }
    let mut compute = Vec::new();
    let mut mem = Vec::new();
    for (i, iv) in inv.iter().enumerate() {
        secs[i].sort_by(|a, b| a.partial_cmp(b).unwrap());
        compute.push(CompStat {
            lang: iv.lang.clone(),
            min: secs[i][0],
            median: secs[i][secs[i].len() / 2],
        });
        mem.push(MemStat { lang: iv.lang.clone(), rss_bytes: rss[i] });
    }
    Ok((compute, mem))
}

/// Memory only track for the startup cases, which have no COMPUTE_NS.
fn rss_track(inv: &[Invocation], samples: u32) -> Result<Vec<MemStat>> {
    let mut mem = Vec::new();
    for iv in inv {
        let mut best: u64 = 0;
        for _ in 0..samples {
            let out = iv.timed_command().output()?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            if let Some(b) = parse_rss_bytes(&stderr) {
                best = best.max(b);
            }
        }
        mem.push(MemStat { lang: iv.lang.clone(), rss_bytes: best });
    }
    Ok(mem)
}

fn parse_compute_ns(stderr: &str) -> Option<f64> {
    stderr.lines().find_map(|l| {
        // Some runtimes color `console.error` even when piped, so the marker
        // can arrive wrapped in ANSI escapes. Take the digits after it.
        let start = l.find("COMPUTE_NS")? + "COMPUTE_NS".len();
        let digits: String = l[start..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<f64>().ok()
    })
}

/// Peak RSS from /usr/bin/time output. macOS `-l` prints bytes as
/// `  123456  maximum resident set size`, GNU `-v` prints
/// `Maximum resident set size (kbytes): 123456`.
fn parse_rss_bytes(stderr: &str) -> Option<u64> {
    for l in stderr.lines() {
        let lower = l.to_ascii_lowercase();
        if !lower.contains("maximum resident set size") {
            continue;
        }
        let digits: String = l.chars().filter(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().ok()?;
        return Some(if lower.contains("kbytes") { n * 1024 } else { n });
    }
    None
}

/// Measure the one-time gate. Cold clears the cache before each run so the full
/// `cargo check` runs. Warm runs the same script with the cache already primed.
fn gate_cost(root: &Path, dir: &Path) -> Result<Gate> {
    let script = root.join("bench/cases/hello/case.rs");
    let script = script.to_string_lossy().into_owned();
    let run = format!("rust run {script}");

    // Warm: prime once, then measure with cache hits.
    let prime = Command::new("rust").args(["run", &script]).status()?;
    if !prime.success() {
        bail!("priming the gate cache failed");
    }
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
