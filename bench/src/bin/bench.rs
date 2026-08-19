use num_traits::AsPrimitive;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rustscript_bench::http_server::HttpServer;
use rustscript_bench::sample::{parse_compute_ns, parse_peak_memory_bytes, rotated_indices};
use rustscript_bench::{CaseResult, Gate, LANGS, MemStat, PYTHON, Report, Settings, TimeStat};

#[derive(Clone, Copy)]
enum Input {
    None,
    Size(u64),
    Data(&'static str),
    FileTransform,
    Process(u64),
    Http(u64),
    Automation,
}

struct Case {
    name: &'static str,
    kind: &'static str,
    input: Input,
}

const CASES: &[Case] = &[
    Case {
        name: "hello",
        kind: "startup",
        input: Input::None,
    },
    Case {
        name: "big_script",
        kind: "startup",
        input: Input::None,
    },
    Case {
        name: "multifile_startup",
        kind: "startup",
        input: Input::None,
    },
    Case {
        name: "fib",
        kind: "compute",
        input: Input::Size(27),
    },
    Case {
        name: "sieve",
        kind: "compute",
        input: Input::Size(250_000),
    },
    Case {
        name: "mandelbrot",
        kind: "compute",
        input: Input::Size(140),
    },
    Case {
        name: "collatz",
        kind: "compute",
        input: Input::Size(10_000),
    },
    Case {
        name: "binary_trees",
        kind: "compute",
        input: Input::Size(11),
    },
    Case {
        name: "string_builder",
        kind: "compute",
        input: Input::Size(200_000),
    },
    Case {
        name: "higher_order",
        kind: "compute",
        input: Input::Size(100_000),
    },
    Case {
        name: "sort",
        kind: "compute",
        input: Input::Size(50_000),
    },
    Case {
        name: "sort_key",
        kind: "compute",
        input: Input::Size(50_000),
    },
    Case {
        name: "hashmap_int",
        kind: "compute",
        input: Input::Size(150_000),
    },
    Case {
        name: "nbody",
        kind: "compute",
        input: Input::Size(8_000),
    },
    Case {
        name: "json_serialize",
        kind: "compute",
        input: Input::Size(100_000),
    },
    Case {
        name: "stdout_lines",
        kind: "compute",
        input: Input::Size(20_000),
    },
    Case {
        name: "word_count",
        kind: "compute",
        input: Input::Data("word_count/data.txt"),
    },
    Case {
        name: "json",
        kind: "compute",
        input: Input::Data("json/data.json"),
    },
    Case {
        name: "regex",
        kind: "compute",
        input: Input::Data("word_count/data.txt"),
    },
    Case {
        name: "file_transform",
        kind: "compute",
        input: Input::FileTransform,
    },
    Case {
        name: "process_spawn",
        kind: "compute",
        input: Input::Process(20),
    },
    Case {
        name: "async_tasks",
        kind: "compute",
        input: Input::Size(20),
    },
    Case {
        name: "http_local",
        kind: "compute",
        input: Input::Http(100),
    },
    Case {
        name: "automation",
        kind: "compute",
        input: Input::Automation,
    },
];

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self> {
        let root = env::temp_dir().join(format!("rustscript-bench-{}", process::id()));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn outputs(&self) -> PathBuf {
        self.root.join("outputs")
    }

    fn check_cache(&self) -> PathBuf {
        self.root.join("check-cache")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!(
                "could not remove benchmark scratch {}: {error}",
                self.root.display()
            );
        }
    }
}

fn main() -> Result<()> {
    let quick = env::args().any(|arg| arg == "--quick");
    let samples = sample_override()?.unwrap_or(if quick { 3 } else { 5 });
    let case_filter = case_override()?;
    let settings = Settings {
        warmups: 1,
        total_samples: samples,
        compute_samples: samples,
        quick,
    };
    let root = workspace_root()?;
    let scratch = Scratch::new()?;
    fs::create_dir_all(scratch.outputs())?;

    ensure_tool("node")?;
    ensure_tool(PYTHON)?;
    generate_fixtures(&root)?;
    build_binaries(&root)?;

    let rustscript = root.join("target/release/rust");
    let helper = root.join("target/release/bench-child");
    let server = root.join("target/release/bench-server");
    let mut results = Vec::new();
    for case in CASES {
        if let Some(name) = &case_filter
            && case.name != name
        {
            continue;
        }
        println!("\n== {} ==", case.name);
        let mut http = if matches!(case.input, Input::Http(_)) {
            Some(HttpServer::start(&server)?)
        } else {
            None
        };
        let url = http.as_ref().map(HttpServer::url);
        let context = InvocationContext {
            root: &root,
            scratch: &scratch,
            rustscript: &rustscript,
            helper: &helper,
            server_url: url,
        };
        let invocations: Vec<Invocation> = LANGS
            .iter()
            .map(|lang| invocation(&context, case, lang))
            .collect::<Result<_>>()?;
        gate_check(case, &invocations)?;
        let total = total_track(&invocations, &settings)?;
        let (compute, memory) = if case.kind == "compute" {
            compute_track(&invocations, settings.compute_samples)?
        } else {
            (
                Vec::new(),
                memory_track(&invocations, settings.compute_samples)?,
            )
        };
        print_stats(&total, &compute, &memory);
        results.push(CaseResult {
            name: case.name.to_string(),
            kind: case.kind.to_string(),
            parameters: case_parameters(case),
            total,
            compute,
            memory,
        });
        if let Some(running) = http.take() {
            running.stop()?;
        }
    }

    let results_dir = root.join("bench/results");
    fs::create_dir_all(&results_dir)?;
    let output = results_dir.join("results.json");
    // A filtered run replaces only its cases in the existing report, so the
    // other cases and the recorded provenance stay from the full run.
    let report = if case_filter.is_some() {
        let mut report: Report = serde_json::from_str(&fs::read_to_string(&output)?)?;
        for fresh in results {
            let Some(slot) = report
                .cases
                .iter_mut()
                .find(|existing| existing.name == fresh.name)
            else {
                bail!("case {} is not in the existing report", fresh.name);
            };
            *slot = fresh;
        }
        report
    } else {
        println!("\n== warm check ==");
        let gate = warm_check(&root, &scratch, &rustscript, &settings)?;
        println!("  warm check {:>8.2} ms", gate.warm_median * 1e3);
        let fixtures = fixture_paths(&root);
        let meta = rustscript_bench::provenance::gather(&root, &rustscript, &fixtures, settings)?;
        Report {
            schema_version: 4,
            meta,
            cases: results,
            gate,
        }
    };
    fs::write(&output, serde_json::to_string_pretty(&report)?)?;
    println!("\nwrote {}", output.display());
    println!("now run: cargo run --release --bin chart");
    Ok(())
}

fn case_override() -> Result<Option<String>> {
    let args: Vec<String> = env::args().collect();
    for pair in args.windows(2) {
        if pair[0] == "--case" {
            let name = pair[1].clone();
            if !CASES.iter().any(|case| case.name == name) {
                bail!("unknown case {name}");
            }
            return Ok(Some(name));
        }
    }
    Ok(None)
}

fn sample_override() -> Result<Option<u32>> {
    let args: Vec<String> = env::args().collect();
    for pair in args.windows(2) {
        if pair[0] == "--samples" {
            let samples: u32 = pair[1].parse()?;
            if samples == 0 {
                bail!("--samples must be positive");
            }
            return Ok(Some(samples));
        }
    }
    Ok(None)
}

fn generate_fixtures(root: &Path) -> Result<()> {
    println!("generating deterministic fixtures ...");
    let status = Command::new(env!("CARGO"))
        .args(["run", "--release", "--bin", "gendata"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("fixture generation failed");
    }
    Ok(())
}

fn build_binaries(root: &Path) -> Result<()> {
    println!("building workspace rustscript and benchmark binaries ...");
    run_cargo(root, &["build", "--release", "-p", "run-rs"])?;
    run_cargo(
        root,
        &["build", "--release", "-p", "rustscript-bench", "--bins"],
    )
}

fn run_cargo(root: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
}

struct Invocation {
    lang: String,
    program: PathBuf,
    args: Vec<String>,
    output_file: Option<PathBuf>,
}

struct InvocationContext<'a> {
    root: &'a Path,
    scratch: &'a Scratch,
    rustscript: &'a Path,
    helper: &'a Path,
    server_url: Option<&'a str>,
}

impl Invocation {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).stdin(Stdio::null());
        command
    }

    fn timed_command(&self) -> Command {
        let mut command = Command::new("/usr/bin/time");
        command.arg(if cfg!(target_os = "macos") {
            "-l"
        } else {
            "-v"
        });
        command
            .arg(&self.program)
            .args(&self.args)
            .stdin(Stdio::null());
        command
    }
}

fn invocation(context: &InvocationContext<'_>, case: &Case, lang: &str) -> Result<Invocation> {
    let case_dir = context.root.join("bench/cases").join(case.name);
    let (case_args, output_file) = case_args(
        context.root,
        context.scratch,
        context.helper,
        case,
        lang,
        context.server_url,
    )?;
    let (program, mut args) = match lang {
        "native" => (
            context.root.join("target/release").join(case.name),
            Vec::new(),
        ),
        "rustscript" => (
            context.rustscript.to_path_buf(),
            vec![case_dir.join("case.rs").display().to_string()],
        ),
        "node" => (
            PathBuf::from("node"),
            vec![case_dir.join("case.ts").display().to_string()],
        ),
        "python" => (
            PathBuf::from(PYTHON),
            vec![case_dir.join("case.py").display().to_string()],
        ),
        _ => unreachable!(),
    };
    args.extend(case_args);
    Ok(Invocation {
        lang: lang.to_string(),
        program,
        args,
        output_file,
    })
}

fn case_args(
    root: &Path,
    scratch: &Scratch,
    helper: &Path,
    case: &Case,
    lang: &str,
    server_url: Option<&str>,
) -> Result<(Vec<String>, Option<PathBuf>)> {
    let words = || root.join("bench/cases/word_count/data.txt");
    let output = || {
        scratch
            .outputs()
            .join(format!("{}_{}.out", case.name, lang))
    };
    let result = match case.input {
        Input::None => (Vec::new(), None),
        Input::Size(size) => (vec![size.to_string()], None),
        Input::Data(fixture) => (
            vec![root.join("bench/cases").join(fixture).display().to_string()],
            None,
        ),
        Input::FileTransform => {
            let destination = output();
            (
                vec![
                    words().display().to_string(),
                    destination.display().to_string(),
                ],
                Some(destination),
            )
        }
        Input::Process(runs) => (vec![helper.display().to_string(), runs.to_string()], None),
        Input::Http(requests) => {
            let url = server_url.context("HTTP case needs server")?;
            (vec![url.to_string(), requests.to_string()], None)
        }
        Input::Automation => {
            let destination = output();
            (
                vec![
                    root.join("bench/cases/automation/config.json")
                        .display()
                        .to_string(),
                    words().display().to_string(),
                    destination.display().to_string(),
                ],
                Some(destination),
            )
        }
    };
    Ok(result)
}

fn case_parameters(case: &Case) -> Vec<String> {
    match case.input {
        Input::None => Vec::new(),
        Input::Size(size) => vec![format!("size={size}")],
        Input::Data(fixture) => vec![format!("fixture={fixture}")],
        Input::FileTransform => vec!["fixture=word_count/data.txt".to_string()],
        Input::Process(runs) => vec![format!("helper_runs={runs}")],
        Input::Http(requests) => vec![format!("requests={requests}")],
        Input::Automation => vec![
            "fixture=word_count/data.txt".to_string(),
            "top=20".to_string(),
        ],
    }
}

fn gate_check(case: &Case, invocations: &[Invocation]) -> Result<()> {
    let mut stdout: Option<(String, Vec<u8>)> = None;
    let mut output_file: Option<(String, Vec<u8>)> = None;
    for invocation in invocations {
        if let Some(path) = &invocation.output_file
            && path.exists()
        {
            fs::remove_file(path)?;
        }
        let output = invocation.command().output()?;
        if !output.status.success() {
            bail!(
                "{} {} failed:\n{}",
                invocation.lang,
                case.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        compare_bytes(case.name, &invocation.lang, &mut stdout, output.stdout)?;
        if let Some(path) = &invocation.output_file {
            compare_bytes(
                case.name,
                &invocation.lang,
                &mut output_file,
                fs::read(path)?,
            )?;
        }
    }
    println!("  gate   ok, all four match");
    Ok(())
}

fn compare_bytes(
    case: &str,
    lang: &str,
    baseline: &mut Option<(String, Vec<u8>)>,
    bytes: Vec<u8>,
) -> Result<()> {
    match baseline {
        None => *baseline = Some((lang.to_string(), bytes)),
        Some((baseline_lang, baseline_bytes)) if *baseline_bytes != bytes => {
            bail!("output mismatch for {case}: {baseline_lang} vs {lang}");
        }
        Some(_) => {}
    }
    Ok(())
}

fn total_track(invocations: &[Invocation], settings: &Settings) -> Result<Vec<TimeStat>> {
    for invocation in invocations {
        for _ in 0..settings.warmups {
            run_total(invocation)?;
        }
    }
    let mut samples = vec![Vec::new(); invocations.len()];
    for round in 0..settings.total_samples as usize {
        for index in rotated_indices(invocations.len(), round) {
            samples[index].push(run_total(&invocations[index])?);
        }
    }
    Ok(invocations
        .iter()
        .zip(samples)
        .map(|(invocation, values)| TimeStat::from_samples(&invocation.lang, values))
        .collect())
}

fn run_total(invocation: &Invocation) -> Result<f64> {
    let start = Instant::now();
    let status = invocation
        .command()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let elapsed = start.elapsed().as_secs_f64();
    if !status.success() {
        bail!("total time run failed for {}", invocation.lang);
    }
    Ok(elapsed)
}

fn compute_track(invocations: &[Invocation], count: u32) -> Result<(Vec<TimeStat>, Vec<MemStat>)> {
    let mut times = vec![Vec::new(); invocations.len()];
    let mut memory = vec![Vec::new(); invocations.len()];
    for round in 0..count as usize {
        for index in rotated_indices(invocations.len(), round) {
            let invocation = &invocations[index];
            let output = invocation.timed_command().stdout(Stdio::null()).output()?;
            if !output.status.success() {
                bail!("compute run failed for {}", invocation.lang);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let ns = parse_compute_ns(&stderr).context("missing COMPUTE_NS")?;
            let peak = parse_peak_memory_bytes(&stderr).context("missing peak memory")?;
            times[index].push(ns / 1e9);
            memory[index].push(peak);
        }
    }
    Ok((
        invocations
            .iter()
            .zip(times)
            .map(|(invocation, values)| TimeStat::from_samples(&invocation.lang, values))
            .collect(),
        invocations
            .iter()
            .zip(memory)
            .map(|(invocation, values)| MemStat::from_samples(&invocation.lang, values))
            .collect(),
    ))
}

fn memory_track(invocations: &[Invocation], count: u32) -> Result<Vec<MemStat>> {
    let mut memory = vec![Vec::new(); invocations.len()];
    for round in 0..count as usize {
        for index in rotated_indices(invocations.len(), round) {
            let invocation = &invocations[index];
            let output = invocation.timed_command().stdout(Stdio::null()).output()?;
            if !output.status.success() {
                bail!("memory run failed for {}", invocation.lang);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            memory[index].push(parse_peak_memory_bytes(&stderr).context("missing peak memory")?);
        }
    }
    Ok(invocations
        .iter()
        .zip(memory)
        .map(|(invocation, values)| MemStat::from_samples(&invocation.lang, values))
        .collect())
}

fn warm_check(
    root: &Path,
    scratch: &Scratch,
    rustscript: &Path,
    settings: &Settings,
) -> Result<Gate> {
    let script = root.join("bench/cases/hello/case.rs");
    fs::create_dir_all(scratch.check_cache())?;
    let run = || {
        let mut command = Command::new(rustscript);
        command
            .args(["check", &script.display().to_string()])
            .env("XDG_CACHE_HOME", scratch.check_cache())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    };
    if !run().status()?.success() {
        bail!("priming warm check failed");
    }
    for _ in 0..settings.warmups {
        if !run().status()?.success() {
            bail!("warm check failed");
        }
    }
    let mut samples = Vec::new();
    for _ in 0..settings.total_samples {
        let start = Instant::now();
        let status = run().status()?;
        samples.push(start.elapsed().as_secs_f64());
        if !status.success() {
            bail!("warm check failed");
        }
    }
    let stat = TimeStat::from_samples("rustscript", samples);
    Ok(Gate {
        warm_median: stat.median,
        warm_samples: stat.samples,
    })
}

fn print_stats(total: &[TimeStat], compute: &[TimeStat], memory: &[MemStat]) {
    for stat in total {
        println!("  total  {:<11} {:>8.2} ms", stat.lang, stat.median * 1e3);
    }
    for stat in compute {
        println!("  compute{:<11} {:>8.2} ms", stat.lang, stat.median * 1e3);
    }
    for stat in memory {
        println!(
            "  memory {:<11} {:>8.1} MB",
            stat.lang,
            AsPrimitive::<f64>::as_(stat.median_bytes) / 1e6
        );
    }
}

fn fixture_paths(root: &Path) -> Vec<(String, PathBuf)> {
    vec![
        (
            "words".to_string(),
            root.join("bench/cases/word_count/data.txt"),
        ),
        ("json".to_string(), root.join("bench/cases/json/data.json")),
        (
            "automation_config".to_string(),
            root.join("bench/cases/automation/config.json"),
        ),
    ]
}

fn workspace_root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no workspace parent")?
        .to_path_buf())
}

fn ensure_tool(name: &str) -> Result<()> {
    let found = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !found {
        bail!("required tool `{name}` not found on PATH");
    }
    Ok(())
}
