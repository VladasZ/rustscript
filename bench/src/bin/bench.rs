use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rustscript_bench::http_server::HttpServer;
use rustscript_bench::sample::{parse_compute_ns, parse_rss_bytes, rotated_indices};
use rustscript_bench::{CaseResult, Gate, LANGS, MemStat, Report, Settings, TimeStat};

#[derive(Clone, Copy)]
enum Input {
    None,
    Size { base: u64, big: u64 },
    Data { base: &'static str, big: BigFixture },
    FileTransform,
    Process { base: u64, big: u64 },
    Http { base: u64, big: u64 },
    Automation,
}

#[derive(Clone, Copy)]
enum BigFixture {
    Words,
    Json,
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
        input: Input::Size { base: 27, big: 32 },
    },
    Case {
        name: "sieve",
        kind: "compute",
        input: Input::Size {
            base: 250_000,
            big: 2_500_000,
        },
    },
    Case {
        name: "mandelbrot",
        kind: "compute",
        input: Input::Size {
            base: 140,
            big: 440,
        },
    },
    Case {
        name: "collatz",
        kind: "compute",
        input: Input::Size {
            base: 10_000,
            big: 100_000,
        },
    },
    Case {
        name: "binary_trees",
        kind: "compute",
        input: Input::Size { base: 11, big: 14 },
    },
    Case {
        name: "string_builder",
        kind: "compute",
        input: Input::Size {
            base: 200_000,
            big: 2_000_000,
        },
    },
    Case {
        name: "higher_order",
        kind: "compute",
        input: Input::Size {
            base: 100_000,
            big: 1_000_000,
        },
    },
    Case {
        name: "sort",
        kind: "compute",
        input: Input::Size {
            base: 50_000,
            big: 500_000,
        },
    },
    Case {
        name: "hashmap_int",
        kind: "compute",
        input: Input::Size {
            base: 150_000,
            big: 1_500_000,
        },
    },
    Case {
        name: "nbody",
        kind: "compute",
        input: Input::Size {
            base: 8_000,
            big: 80_000,
        },
    },
    Case {
        name: "json_serialize",
        kind: "compute",
        input: Input::Size {
            base: 100_000,
            big: 1_000_000,
        },
    },
    Case {
        name: "stdout_lines",
        kind: "compute",
        input: Input::Size {
            base: 20_000,
            big: 200_000,
        },
    },
    Case {
        name: "word_count",
        kind: "compute",
        input: Input::Data {
            base: "word_count/data.txt",
            big: BigFixture::Words,
        },
    },
    Case {
        name: "json",
        kind: "compute",
        input: Input::Data {
            base: "json/data.json",
            big: BigFixture::Json,
        },
    },
    Case {
        name: "regex",
        kind: "compute",
        input: Input::Data {
            base: "word_count/data.txt",
            big: BigFixture::Words,
        },
    },
    Case {
        name: "file_transform",
        kind: "compute",
        input: Input::FileTransform,
    },
    Case {
        name: "process_spawn",
        kind: "compute",
        input: Input::Process { base: 20, big: 200 },
    },
    Case {
        name: "async_tasks",
        kind: "compute",
        input: Input::Size { base: 20, big: 200 },
    },
    Case {
        name: "http_local",
        kind: "compute",
        input: Input::Http {
            base: 100,
            big: 1_000,
        },
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

    fn fixtures(&self) -> PathBuf {
        self.root.join("fixtures")
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
    let samples = sample_override()?.unwrap_or(if quick { 3 } else { 10 });
    let settings = Settings {
        warmups: 3,
        wall_samples: samples,
        compute_samples: samples,
        quick,
    };
    let root = workspace_root()?;
    let scratch = Scratch::new()?;
    fs::create_dir_all(scratch.outputs())?;

    ensure_tool("node")?;
    ensure_tool("python3")?;
    generate_fixtures(&root, &scratch)?;
    build_binaries(&root)?;

    let rustscript = root.join("target/release/rust");
    let helper = root.join("target/release/bench-child");
    let server = root.join("target/release/bench-server");
    let mut results = Vec::new();
    for case in CASES {
        for tier in ["base", "big"] {
            if tier == "big" && matches!(case.input, Input::None) {
                continue;
            }
            println!("\n== {} {} ==", case.name, tier);
            let mut http = if matches!(case.input, Input::Http { .. }) {
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
                .map(|lang| invocation(&context, case, tier, lang))
                .collect::<Result<_>>()?;
            gate_check(case, &invocations)?;
            let wall = wall_track(&invocations, &settings)?;
            let (compute, memory) = if case.kind == "compute" {
                compute_track(&invocations, settings.compute_samples)?
            } else {
                (
                    Vec::new(),
                    memory_track(&invocations, settings.compute_samples)?,
                )
            };
            print_stats(&wall, &compute, &memory);
            results.push(CaseResult {
                name: case.name.to_string(),
                kind: case.kind.to_string(),
                tier: tier.to_string(),
                parameters: case_parameters(case, tier),
                wall,
                compute,
                memory,
            });
            if let Some(running) = http.take() {
                running.stop()?;
            }
        }
    }

    println!("\n== warm check ==");
    let gate = warm_check(&root, &scratch, &rustscript, &settings)?;
    println!("  warm check {:>8.2} ms", gate.warm_median * 1e3);
    let fixtures = fixture_paths(&root, &scratch);
    let meta = rustscript_bench::provenance::gather(&root, &rustscript, &fixtures, settings)?;
    let report = Report {
        schema_version: 2,
        meta,
        cases: results,
        gate,
    };
    let results_dir = root.join("bench/results");
    fs::create_dir_all(&results_dir)?;
    let output = results_dir.join("results.json");
    fs::write(&output, serde_json::to_string_pretty(&report)?)?;
    println!("\nwrote {}", output.display());
    println!("now run: cargo run --release --bin chart");
    Ok(())
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

fn generate_fixtures(root: &Path, scratch: &Scratch) -> Result<()> {
    println!("generating deterministic fixtures ...");
    let status = Command::new(env!("CARGO"))
        .args(["run", "--release", "--bin", "gendata", "--"])
        .arg(scratch.fixtures())
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("fixture generation failed");
    }
    Ok(())
}

fn build_binaries(root: &Path) -> Result<()> {
    println!("building workspace rustscript and benchmark binaries ...");
    run_cargo(root, &["build", "--release", "-p", "rustscript"])?;
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

fn invocation(
    context: &InvocationContext<'_>,
    case: &Case,
    tier: &str,
    lang: &str,
) -> Result<Invocation> {
    let case_dir = context.root.join("bench/cases").join(case.name);
    let (case_args, output_file) = case_args(
        context.root,
        context.scratch,
        context.helper,
        case,
        tier,
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
            vec![
                "run".to_string(),
                case_dir.join("case.rs").display().to_string(),
            ],
        ),
        "node" => (
            PathBuf::from("node"),
            vec![case_dir.join("case.ts").display().to_string()],
        ),
        "python" => (
            PathBuf::from("python3"),
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
    tier: &str,
    lang: &str,
    server_url: Option<&str>,
) -> Result<(Vec<String>, Option<PathBuf>)> {
    let is_big = tier == "big";
    let size = |base: u64, big: u64| if is_big { big } else { base };
    let words = || {
        if is_big {
            scratch.fixtures().join("word_count/data_big.txt")
        } else {
            root.join("bench/cases/word_count/data.txt")
        }
    };
    let output = || {
        scratch
            .outputs()
            .join(format!("{}_{}_{}.out", case.name, tier, lang))
    };
    let result = match case.input {
        Input::None => (Vec::new(), None),
        Input::Size { base, big } => (vec![size(base, big).to_string()], None),
        Input::Data { base, big } => {
            let path = if is_big {
                match big {
                    BigFixture::Words => scratch.fixtures().join("word_count/data_big.txt"),
                    BigFixture::Json => scratch.fixtures().join("json/data_big.json"),
                }
            } else {
                root.join("bench/cases").join(base)
            };
            (vec![path.display().to_string()], None)
        }
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
        Input::Process { base, big } => (
            vec![helper.display().to_string(), size(base, big).to_string()],
            None,
        ),
        Input::Http { base, big } => {
            let url = server_url.context("HTTP case needs server")?;
            (vec![url.to_string(), size(base, big).to_string()], None)
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

fn case_parameters(case: &Case, tier: &str) -> Vec<String> {
    let is_big = tier == "big";
    let size = |base: u64, big: u64| if is_big { big } else { base };
    let fixture = if is_big { "words_big" } else { "words_base" };
    match case.input {
        Input::None => Vec::new(),
        Input::Size { base, big } => vec![format!("size={}", size(base, big))],
        Input::Data { big, .. } => {
            let fixture = match (is_big, big) {
                (false, BigFixture::Words) => "words_base",
                (true, BigFixture::Words) => "words_big",
                (false, BigFixture::Json) => "json_base",
                (true, BigFixture::Json) => "json_big",
            };
            vec![format!("fixture={fixture}")]
        }
        Input::FileTransform => vec![format!("fixture={fixture}")],
        Input::Process { base, big } => {
            vec![format!("helper_runs={}", size(base, big))]
        }
        Input::Http { base, big } => vec![format!("requests={}", size(base, big))],
        Input::Automation => vec![format!("fixture={fixture}"), "top=20".to_string()],
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

fn wall_track(invocations: &[Invocation], settings: &Settings) -> Result<Vec<TimeStat>> {
    for invocation in invocations {
        for _ in 0..settings.warmups {
            run_wall(invocation)?;
        }
    }
    let mut samples = vec![Vec::new(); invocations.len()];
    for round in 0..settings.wall_samples as usize {
        for index in rotated_indices(invocations.len(), round) {
            samples[index].push(run_wall(&invocations[index])?);
        }
    }
    Ok(invocations
        .iter()
        .zip(samples)
        .map(|(invocation, values)| TimeStat::from_samples(&invocation.lang, values))
        .collect())
}

fn run_wall(invocation: &Invocation) -> Result<f64> {
    let start = Instant::now();
    let status = invocation
        .command()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let elapsed = start.elapsed().as_secs_f64();
    if !status.success() {
        bail!("wall run failed for {}", invocation.lang);
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
            let rss = parse_rss_bytes(&stderr).context("missing maximum RSS")?;
            times[index].push(ns / 1e9);
            memory[index].push(rss);
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
            memory[index].push(parse_rss_bytes(&stderr).context("missing maximum RSS")?);
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
    for _ in 0..settings.wall_samples {
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

fn print_stats(wall: &[TimeStat], compute: &[TimeStat], memory: &[MemStat]) {
    for stat in wall {
        println!("  wall   {:<11} {:>8.2} ms", stat.lang, stat.median * 1e3);
    }
    for stat in compute {
        println!("  compute{:<11} {:>8.2} ms", stat.lang, stat.median * 1e3);
    }
    for stat in memory {
        println!(
            "  rss    {:<11} {:>8.1} MB",
            stat.lang,
            stat.median_bytes as f64 / 1e6
        );
    }
}

fn fixture_paths(root: &Path, scratch: &Scratch) -> Vec<(String, PathBuf)> {
    vec![
        (
            "words_base".to_string(),
            root.join("bench/cases/word_count/data.txt"),
        ),
        (
            "words_big".to_string(),
            scratch.fixtures().join("word_count/data_big.txt"),
        ),
        (
            "json_base".to_string(),
            root.join("bench/cases/json/data.json"),
        ),
        (
            "json_big".to_string(),
            scratch.fixtures().join("json/data_big.json"),
        ),
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
