use std::process::ExitCode;
use std::time::{Duration, Instant};

use rustscript_differential::artifact::Artifact;
use rustscript_differential::generator::generate;
use rustscript_differential::model::Program;
use rustscript_differential::mutator::mutate;
use rustscript_differential::reduce::{ReductionProgress, reduce_with_progress};
use rustscript_differential::runner::{Classification, RunResult, Runner};
use rustscript_differential::workspace_root;

const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const CAMPAIGN_BATCH_SIZE: usize = 8;

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("differential error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };
    match command {
        "run" => run_campaign(&args[1..]),
        "generate" => generate_one(&args[1..]),
        "mutate" => mutate_artifact(&args[1..]),
        "replay" => replay(&args[1..]),
        "reduce" => reduce_artifact(&args[1..]),
        "promote" => promote(&args[1..]),
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => anyhow::bail!("unknown command `{other}`"),
    }
}

fn run_campaign(args: &[String]) -> anyhow::Result<()> {
    let mut seed = 0;
    let mut cases = 100;
    let mut timeout_ms = 2_000;
    let mut stop_on_first = false;
    let mut index = 0;
    while index < args.len() {
        let option = &args[index];
        if option == "--stop-on-first" {
            stop_on_first = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("missing value after `{option}`"))?;
        match option.as_str() {
            "--seed" => seed = value.parse()?,
            "--cases" => cases = value.parse()?,
            "--timeout-ms" => timeout_ms = value.parse()?,
            other => anyhow::bail!("unknown run option `{other}`"),
        }
        index += 2;
    }

    let root = workspace_root();
    let runner = Runner::build(&root, timeout_ms)?;
    let mut report = CampaignReport::default();
    let started = Instant::now();
    let mut last_progress = started;
    println!("running {cases} cases from seed {seed}");

    let mut offset = 0;
    'campaign: while offset < cases {
        let batch_size = CAMPAIGN_BATCH_SIZE.min(cases - offset);
        let programs = (0..batch_size)
            .map(|batch_offset| generate(seed + (offset + batch_offset) as u64))
            .collect::<Vec<_>>();
        let sources = programs.iter().map(Program::render).collect::<Vec<_>>();
        let results = runner.run_sources(&sources)?;

        for ((program, source), result) in programs.into_iter().zip(sources).zip(results) {
            let case_seed = program.seed;
            match &result.classification {
                Classification::Match => {
                    report.matched += 1;
                    if report.matched % 100 == 0 || last_progress.elapsed() >= PROGRESS_INTERVAL {
                        print_campaign_progress(
                            report.matched,
                            cases,
                            case_seed,
                            started.elapsed(),
                        );
                        last_progress = Instant::now();
                    }
                }
                Classification::InterpreterUnsupported => {
                    report.record_gap(&result);
                }
                classification => {
                    if stop_on_first {
                        return stop_and_reduce(&runner, &root, case_seed, program, source, result);
                    }
                    let key = format!("{classification:?}");
                    let saved = report.should_save(&key);
                    let path = if saved {
                        Some(
                            Artifact::new(case_seed, program, source, result.clone())
                                .save(&root)?,
                        )
                    } else {
                        None
                    };
                    report.record_bug(key, case_seed, path);
                }
            }
            report.checked += 1;
            if report.checked >= cases {
                break 'campaign;
            }
        }
        offset += batch_size;
    }

    report.print(started.elapsed());
    Ok(())
}

const MAX_SEEDS_PER_GROUP: usize = 8;
const MAX_ARTIFACTS_PER_GROUP: usize = 3;

#[derive(Default)]
struct CampaignReport {
    checked: usize,
    matched: usize,
    gaps: std::collections::BTreeMap<String, usize>,
    bugs: std::collections::BTreeMap<String, BugGroup>,
}

#[derive(Default)]
struct BugGroup {
    count: usize,
    seeds: Vec<u64>,
    artifacts: Vec<std::path::PathBuf>,
}

impl CampaignReport {
    fn record_gap(&mut self, result: &RunResult) {
        let message = gap_message(&result.interpreted.stderr);
        *self.gaps.entry(message).or_default() += 1;
    }

    fn should_save(&self, key: &str) -> bool {
        self.bugs
            .get(key)
            .is_none_or(|group| group.artifacts.len() < MAX_ARTIFACTS_PER_GROUP)
    }

    fn record_bug(&mut self, key: String, seed: u64, path: Option<std::path::PathBuf>) {
        let group = self.bugs.entry(key).or_default();
        group.count += 1;
        if group.seeds.len() < MAX_SEEDS_PER_GROUP {
            group.seeds.push(seed);
        }
        if let Some(path) = path {
            group.artifacts.push(path);
        }
    }

    fn print(&self, elapsed: Duration) {
        let findings: usize = self.bugs.values().map(|group| group.count).sum();
        let gaps: usize = self.gaps.values().sum();
        println!(
            "\nchecked {}: {} matched, {} findings, {} gaps, {:.1}s",
            self.checked,
            self.matched,
            findings,
            gaps,
            elapsed.as_secs_f64()
        );
        if !self.bugs.is_empty() {
            println!("\nfindings (real divergences):");
            for (kind, group) in &self.bugs {
                let seeds = group
                    .seeds
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  {kind}: {} case(s), seeds {seeds}", group.count);
                for path in &group.artifacts {
                    if let Some(dir) = path.parent() {
                        println!("    saved {}", dir.display());
                    }
                }
            }
        }
        if !self.gaps.is_empty() {
            println!("\ngaps (features the interpreter does not run yet):");
            for (message, count) in &self.gaps {
                println!("  {count} case(s): {message}");
            }
        }
    }
}

/// The first meaningful line of an interpreter error, used to group gaps by the
/// missing feature rather than by the exact values in the message.
fn gap_message(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "Error:")
        .unwrap_or("unknown gap")
        .to_string()
}

fn stop_and_reduce(
    runner: &Runner,
    root: &std::path::Path,
    case_seed: u64,
    program: Program,
    source: String,
    result: RunResult,
) -> anyhow::Result<()> {
    let artifact = Artifact::new(case_seed, program, source, result);
    let path = artifact.save(root)?;
    println!("seed {case_seed} differs; minimizing the saved case");
    let (program, result) =
        reduce_with_cli_progress(runner, &artifact.program, &artifact.result.classification)?;
    let reduced = Artifact::new(case_seed, program.clone(), program.render(), result);
    let artifact_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("artifact has no parent directory"))?;
    let reduced_path = reduced.save_under(artifact_dir, "reduced")?;
    anyhow::bail!(
        "seed {case_seed} is {:?}; reduced artifact at {}",
        artifact.result.classification,
        reduced_path.display(),
    );
}

fn generate_one(args: &[String]) -> anyhow::Result<()> {
    let seed = parse_seed(args)?;
    print!("{}", generate(seed).render());
    Ok(())
}

fn mutate_artifact(args: &[String]) -> anyhow::Result<()> {
    let [path, option, seed] = args else {
        anyhow::bail!("usage: rustscript-differential mutate ARTIFACT --seed SEED");
    };
    if option != "--seed" {
        anyhow::bail!("usage: rustscript-differential mutate ARTIFACT --seed SEED");
    }
    let seed = seed.parse()?;
    let artifact = Artifact::load(&std::path::PathBuf::from(path))?;
    print!(
        "{}",
        mutate(&artifact.program, artifact.seed, seed, seed).render()
    );
    Ok(())
}

fn replay(args: &[String]) -> anyhow::Result<()> {
    let path = required_path(args, "replay")?;
    let artifact = Artifact::load(&path)?;
    let runner = Runner::build(&workspace_root(), 2_000)?;
    let result = runner.run_source(&artifact.source)?;
    println!("{:#?}", result.classification);
    print_outputs(&result);
    Ok(())
}

fn reduce_artifact(args: &[String]) -> anyhow::Result<()> {
    let path = required_path(args, "reduce")?;
    let artifact = Artifact::load(&path)?;
    let runner = Runner::build(&workspace_root(), 2_000)?;
    let current = runner.run_source(&artifact.source)?;
    if !current
        .classification
        .same_failure(&artifact.result.classification)
    {
        anyhow::bail!(
            "failure changed from {:?} to {:?}",
            artifact.result.classification,
            current.classification
        );
    }
    let (program, result) =
        reduce_with_cli_progress(&runner, &artifact.program, &current.classification)?;
    let reduced = Artifact::new(artifact.seed, program.clone(), program.render(), result);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("artifact has no parent directory"))?;
    let saved = reduced.save_under(parent, "reduced")?;
    println!("reduced artifact saved at {}", saved.display());
    Ok(())
}

fn print_campaign_progress(completed: usize, total: usize, seed: u64, elapsed: Duration) {
    let rate = completed as f64 / elapsed.as_secs_f64();
    println!(
        "progress: {completed}/{total}, through seed {seed}, {rate:.1} cases/s, {:.1}s elapsed",
        elapsed.as_secs_f64()
    );
}

fn reduce_with_cli_progress(
    runner: &Runner,
    program: &Program,
    target: &Classification,
) -> anyhow::Result<(Program, RunResult)> {
    let started = Instant::now();
    let mut last_progress = started;
    let mut final_progress = ReductionProgress::default();
    let result = reduce_with_progress(runner, program, target, |progress| {
        final_progress = progress;
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            println!(
                "minimizing: {} checked, {} kept, {} cached, {:.1}s elapsed",
                progress.candidates_checked,
                progress.reductions_kept,
                progress.cache_hits,
                started.elapsed().as_secs_f64()
            );
            last_progress = Instant::now();
        }
    })?;
    println!(
        "minimization complete: {} checked, {} kept, {} cached, {:.1}s elapsed",
        final_progress.candidates_checked,
        final_progress.reductions_kept,
        final_progress.cache_hits,
        started.elapsed().as_secs_f64()
    );
    Ok(result)
}

fn promote(args: &[String]) -> anyhow::Result<()> {
    if args.len() != 2 {
        anyhow::bail!("usage: rustscript-differential promote ARTIFACT NAME");
    }
    let path = std::path::PathBuf::from(&args[0]);
    let name = &args[1];
    validate_name(name)?;
    let artifact = Artifact::load(&path)?;
    let root = workspace_root();
    let runner = Runner::build(&root, 2_000)?;
    let current = runner.run_source(&artifact.source)?;
    if current.classification != Classification::Match {
        anyhow::bail!(
            "the case is still {:?}; fix RustScript before promotion",
            current.classification
        );
    }
    let destination = root
        .join("crates/examples/examples")
        .join(format!("{name}.rs"));
    if destination.exists() {
        anyhow::bail!("{} already exists", destination.display());
    }
    std::fs::write(&destination, &artifact.source)?;
    println!("promoted regression to {}", destination.display());
    Ok(())
}

fn parse_seed(args: &[String]) -> anyhow::Result<u64> {
    match args {
        [option, seed] if option == "--seed" => Ok(seed.parse()?),
        _ => anyhow::bail!("usage: rustscript-differential generate --seed SEED"),
    }
}

fn required_path(args: &[String], command: &str) -> anyhow::Result<std::path::PathBuf> {
    match args {
        [path] => Ok(std::path::PathBuf::from(path)),
        _ => anyhow::bail!("usage: rustscript-differential {command} ARTIFACT"),
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && name.as_bytes()[0].is_ascii_lowercase();
    if !valid {
        anyhow::bail!("test name must start with a lowercase letter and use [a-z0-9_]");
    }
    Ok(())
}

fn print_outputs(result: &rustscript_differential::runner::RunResult) {
    println!("-- native stdout --\n{}", result.native.stdout);
    println!("-- native stderr --\n{}", result.native.stderr);
    println!("-- interpreted stdout --\n{}", result.interpreted.stdout);
    println!("-- interpreted stderr --\n{}", result.interpreted.stderr);
}

fn print_usage() {
    println!(
        r"rustscript-differential

  run [--seed N] [--cases N] [--timeout-ms N] [--stop-on-first]
  generate --seed N
  mutate ARTIFACT --seed N
  replay ARTIFACT
  reduce ARTIFACT
  promote ARTIFACT NAME"
    );
}
