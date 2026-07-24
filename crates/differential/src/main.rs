use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use rustscript_differential::artifact::Artifact;
use rustscript_differential::generator::generate;
use rustscript_differential::model::Program;
use rustscript_differential::mutator::mutate;
use rustscript_differential::quarantine::Quarantine;
use rustscript_differential::reduce::{ReductionProgress, reduce_with_progress};
use rustscript_differential::runner::{Classification, RunResult, Runner};
use rustscript_differential::workspace_root;

const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const CAMPAIGN_BATCH_SIZE: usize = 8;

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("differential error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    };
    match command {
        "run" => return run_campaign(&args[1..]),
        "generate" => generate_one(&args[1..])?,
        "mutate" => mutate_artifact(&args[1..])?,
        "replay" => replay(&args[1..])?,
        "reduce" => reduce_artifact(&args[1..])?,
        "promote" => promote(&args[1..])?,
        "help" | "-h" | "--help" => print_usage(),
        other => anyhow::bail!("unknown command `{other}`"),
    }
    Ok(ExitCode::SUCCESS)
}

struct CampaignOptions {
    seed: u64,
    cases: usize,
    timeout_ms: u64,
    stop_on_first: bool,
}

fn parse_campaign_options(args: &[String]) -> Result<CampaignOptions> {
    let mut options = CampaignOptions {
        seed: 0,
        cases: 100,
        timeout_ms: 2_000,
        stop_on_first: false,
    };
    let mut index = 0;
    while index < args.len() {
        let option = &args[index];
        if option == "--stop-on-first" {
            options.stop_on_first = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("missing value after `{option}`"))?;
        match option.as_str() {
            "--seed" => options.seed = value.parse()?,
            "--cases" => options.cases = value.parse()?,
            "--timeout-ms" => options.timeout_ms = value.parse()?,
            other => anyhow::bail!("unknown run option `{other}`"),
        }
        index += 2;
    }
    Ok(options)
}

/// One generated case that came back from a worker, ready for reporting.
type BatchOutcome = (Vec<Program>, Vec<String>, Result<Vec<RunResult>>);

fn run_campaign(args: &[String]) -> Result<ExitCode> {
    let options = parse_campaign_options(args)?;
    let root = workspace_root();
    let quarantine = Quarantine::load(&root)?;
    let runner = Runner::build(&root, options.timeout_ms)?;
    let started = Instant::now();
    println!("running {} cases from seed {}", options.cases, options.seed);
    if !quarantine.known.is_empty() {
        println!("{} known divergence(s) quarantined", quarantine.known.len());
    }

    let batch_count = options.cases.div_ceil(CAMPAIGN_BATCH_SIZE);
    let workers = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
        .min(batch_count.max(1));
    let next_batch = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<(usize, BatchOutcome)>();

    let report = thread::scope(|scope| -> Result<CampaignReport> {
        for _ in 0..workers {
            let sender = sender.clone();
            let runner = &runner;
            let next_batch = &next_batch;
            let stop = &stop;
            let options = &options;
            scope.spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let batch = next_batch.fetch_add(1, Ordering::Relaxed);
                    if batch >= batch_count {
                        break;
                    }
                    let start = batch * CAMPAIGN_BATCH_SIZE;
                    let size = CAMPAIGN_BATCH_SIZE.min(options.cases - start);
                    let programs: Vec<Program> = (0..size)
                        .map(|offset| generate(options.seed + (start + offset) as u64))
                        .collect();
                    let sources: Vec<String> = programs.iter().map(Program::render).collect();
                    let results = runner.run_sources(&sources);
                    if sender.send((batch, (programs, sources, results))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        // Workers finish batches out of order. Buffer and report strictly in
        // batch order so progress, artifact saving, and stop-on-first behave
        // the same as a sequential run.
        let mut report = CampaignReport::default();
        let mut last_progress = started;
        let mut pending: BTreeMap<usize, BatchOutcome> = BTreeMap::new();
        let mut next_expected = 0usize;
        for (batch, outcome) in receiver {
            pending.insert(batch, outcome);
            while let Some((programs, sources, results)) = pending.remove(&next_expected) {
                next_expected += 1;
                let results = match results {
                    Ok(results) => results,
                    Err(error) => {
                        stop.store(true, Ordering::Relaxed);
                        return Err(error);
                    }
                };
                for ((program, source), result) in programs.into_iter().zip(sources).zip(results) {
                    let case_seed = program.seed;
                    match &result.classification {
                        Classification::Match => {
                            report.matched += 1;
                            if report.matched % 100 == 0
                                || last_progress.elapsed() >= PROGRESS_INTERVAL
                            {
                                print_campaign_progress(
                                    report.matched,
                                    options.cases,
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
                            if let Some(entry) = quarantine.matches(&result) {
                                report
                                    .record_known(bucket_key(classification, &result), &entry.note);
                                report.checked += 1;
                                continue;
                            }
                            if options.stop_on_first {
                                stop.store(true, Ordering::Relaxed);
                                stop_and_reduce(
                                    &runner, &root, case_seed, program, source, result,
                                )?;
                                unreachable!("stop_and_reduce always fails");
                            }
                            let key = bucket_key(classification, &result);
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
                }
            }
        }
        Ok(report)
    })?;

    report.print(started.elapsed());
    Ok(if report.bugs.is_empty() {
        ExitCode::SUCCESS
    } else {
        // Real divergences fail the run so a scheduled campaign can gate on
        // the exit code. Gaps alone stay green.
        ExitCode::FAILURE
    })
}

/// Buckets group findings by the concrete failure, not only its kind, so two
/// different bugs with the same classification are reported separately.
fn bucket_key(classification: &Classification, result: &RunResult) -> String {
    let signature = result.signature();
    if signature.is_empty() {
        format!("{classification:?}")
    } else {
        format!("{classification:?} @ {signature}")
    }
}

const MAX_SEEDS_PER_GROUP: usize = 8;
const MAX_ARTIFACTS_PER_GROUP: usize = 3;

#[derive(Default)]
struct CampaignReport {
    checked: usize,
    matched: usize,
    gaps: BTreeMap<String, usize>,
    bugs: BTreeMap<String, BugGroup>,
    known: BTreeMap<String, KnownGroup>,
}

#[derive(Default)]
struct KnownGroup {
    count: usize,
    note: String,
}

#[derive(Default)]
struct BugGroup {
    count: usize,
    seeds: Vec<u64>,
    artifacts: Vec<std::path::PathBuf>,
}

impl CampaignReport {
    fn record_gap(&mut self, result: &RunResult) {
        *self.gaps.entry(result.signature()).or_default() += 1;
    }

    fn record_known(&mut self, key: String, note: &str) {
        let group = self.known.entry(key).or_default();
        group.count += 1;
        group.note = note.to_string();
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
        let known: usize = self.known.values().map(|group| group.count).sum();
        println!(
            "\nchecked {}: {} matched, {} findings, {} known, {} gaps, {:.1}s",
            self.checked,
            self.matched,
            findings,
            known,
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
        if !self.known.is_empty() {
            println!("\nknown divergences (quarantined, fix and delete the entry):");
            for (kind, group) in &self.known {
                println!("  {kind}: {} case(s), {}", group.count, group.note);
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

fn stop_and_reduce(
    runner: &Runner,
    root: &std::path::Path,
    case_seed: u64,
    program: Program,
    source: String,
    result: RunResult,
) -> Result<()> {
    let artifact = Artifact::new(case_seed, program, source, result);
    let path = artifact.save(root)?;
    println!("seed {case_seed} differs; minimizing the saved case");
    let (program, result) = reduce_with_cli_progress(runner, &artifact.program, &artifact.result)?;
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

fn generate_one(args: &[String]) -> Result<()> {
    let seed = parse_seed(args)?;
    print!("{}", generate(seed).render());
    Ok(())
}

fn mutate_artifact(args: &[String]) -> Result<()> {
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

fn replay(args: &[String]) -> Result<()> {
    let path = required_path(args, "replay")?;
    let artifact = Artifact::load(&path)?;
    let runner = Runner::build(&workspace_root(), 2_000)?;
    let result = runner.run_source(&artifact.source)?;
    println!("{:#?}", result.classification);
    print_outputs(&result);
    Ok(())
}

fn reduce_artifact(args: &[String]) -> Result<()> {
    let path = required_path(args, "reduce")?;
    let artifact = Artifact::load(&path)?;
    let runner = Runner::build(&workspace_root(), 2_000)?;
    let current = runner.run_source(&artifact.source)?;
    if !current.same_failure(&artifact.result) {
        anyhow::bail!(
            "failure changed from {:?} to {:?}",
            artifact.result.classification,
            current.classification
        );
    }
    let (program, result) = reduce_with_cli_progress(&runner, &artifact.program, &current)?;
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
    target: &RunResult,
) -> Result<(Program, RunResult)> {
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

fn promote(args: &[String]) -> Result<()> {
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
    // A case whose correct behavior is a panic cannot live under examples,
    // the equivalence suite requires a clean exit there. It goes into the
    // differential corpus, which compares panicking runs too.
    let destination = if current.native.status == Some(0) {
        root.join("crates/examples/examples")
            .join(format!("{name}.rs"))
    } else {
        root.join("crates/differential/corpus")
            .join(format!("{name}.rs"))
    };
    if destination.exists() {
        anyhow::bail!("{} already exists", destination.display());
    }
    std::fs::write(&destination, &artifact.source)?;
    println!("promoted regression to {}", destination.display());
    Ok(())
}

fn parse_seed(args: &[String]) -> Result<u64> {
    match args {
        [option, seed] if option == "--seed" => Ok(seed.parse()?),
        _ => anyhow::bail!("usage: rustscript-differential generate --seed SEED"),
    }
}

fn required_path(args: &[String], command: &str) -> Result<std::path::PathBuf> {
    match args {
        [path] => Ok(std::path::PathBuf::from(path)),
        _ => anyhow::bail!("usage: rustscript-differential {command} ARTIFACT"),
    }
}

fn validate_name(name: &str) -> Result<()> {
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

fn print_outputs(result: &RunResult) {
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
