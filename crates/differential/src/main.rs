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
    let mut index = 0;
    while index < args.len() {
        let option = &args[index];
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
    let mut matches = 0;
    let started = Instant::now();
    let mut last_progress = started;
    println!("running {cases} cases from seed {seed}");

    let mut offset = 0;
    while offset < cases {
        let batch_size = CAMPAIGN_BATCH_SIZE.min(cases - offset);
        let programs = (0..batch_size)
            .map(|batch_offset| generate(seed + (offset + batch_offset) as u64))
            .collect::<Vec<_>>();
        let sources = programs.iter().map(Program::render).collect::<Vec<_>>();
        let results = runner.run_sources(&sources)?;

        for ((program, source), result) in programs.into_iter().zip(sources).zip(results) {
            let case_seed = program.seed;
            match result.classification {
                Classification::Match => {
                    matches += 1;
                    if matches % 100 == 0 || last_progress.elapsed() >= PROGRESS_INTERVAL {
                        print_campaign_progress(matches, cases, case_seed, started.elapsed());
                        last_progress = Instant::now();
                    }
                }
                _ => {
                    let artifact = Artifact::new(case_seed, program, source, result);
                    let path = artifact.save(&root)?;
                    println!("seed {case_seed} differs; minimizing the saved case");
                    let (program, result) = reduce_with_cli_progress(
                        &runner,
                        &artifact.program,
                        &artifact.result.classification,
                    )?;
                    let reduced =
                        Artifact::new(case_seed, program.clone(), program.render(), result);
                    let artifact_dir = path
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("artifact has no parent directory"))?;
                    let reduced_path = reduced.save_under(artifact_dir, "reduced")?;
                    anyhow::bail!(
                        "stopped after {matches} matches: seed {case_seed} is {:?}; reduced artifact \
                         at {}",
                        artifact.result.classification,
                        reduced_path.display(),
                    );
                }
            }
        }
        offset += batch_size;
    }

    println!("{matches} matched, no findings");
    Ok(())
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

  run [--seed N] [--cases N] [--timeout-ms N]
  generate --seed N
  mutate ARTIFACT --seed N
  replay ARTIFACT
  reduce ARTIFACT
  promote ARTIFACT NAME"
    );
}
