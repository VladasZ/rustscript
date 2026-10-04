//! The dev check spread over the homelab. This machine and every Linux and Windows node beekeeper lists
//! build the working tree and take a share of the campaign seeds, of the test suite and of
//! the sweep over the script repo. It is a dev tool, CI never runs it.
//!
//! A node gets the tree by `rsync`, uncommitted files included, and builds it itself, so a
//! run also proves the change on Linux. Everything on a node runs at the lowest priority, the
//! nodes host game servers and CI runners.

pub(crate) mod node;

use std::cmp::Reverse;
use std::env::{current_exe, var};
use std::fs::read_to_string;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{available_parallelism, scope};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use node::{Node, Os, local};

/// Cases per job in one range. A range costs an `ssh` round trip, so it runs for some seconds.
const CASES_PER_JOB: usize = 24;

/// The same for this machine.
const LOCAL_CASES_PER_JOB: usize = 8;

const NODES_URL: &str = "https://beekeeper.tailf87cbe.ts.net/api/nodes";

/// The test suite in 3 parts, each for one node. The first 2 are the slow ones.
const TEST_SHARDS: [(&str, &str); 3] = [
    ("interpreter tests", "-p run-rs"),
    ("differential tests", "-p rustscript-differential"),
    (
        "other tests",
        "--workspace --exclude run-rs --exclude rustscript-differential",
    ),
];

pub struct Options {
    pub seed: u64,
    pub cases: usize,
    pub timeout_ms: u64,
    pub tests: bool,
    /// the repo whose `#!/usr/bin/env rust` scripts are swept
    pub sweep: Option<PathBuf>,
}

#[derive(Deserialize)]
struct NodeInfo {
    hostname: String,
    os: String,
    state: String,
}

#[derive(Deserialize)]
struct ToolchainFile {
    toolchain: Toolchain,
}

#[derive(Deserialize)]
struct Toolchain {
    channel: String,
}

/// What one machine did.
#[derive(Default)]
struct Outcome {
    name: String,
    lines: Vec<String>,
    failed: bool,
}

impl Outcome {
    fn note(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn fail(&mut self, line: impl Into<String>) {
        self.failed = true;
        self.lines.push(line.into());
    }
}

/// Whether every machine agreed.
pub fn run(root: &Path, options: &Options) -> Result<bool> {
    let started = Instant::now();
    let channel = pinned_channel(root)?;
    let hosts = fleet_nodes()?;
    let names: Vec<&str> = hosts.iter().map(|(host, _)| host.as_str()).collect();
    println!(
        "fleet: this machine and {} nodes, {}",
        hosts.len(),
        names.join(" ")
    );

    let scripts = match &options.sweep {
        Some(repo) => node::script_files(repo)?,
        None => Vec::new(),
    };
    let prepared: Vec<Result<Node>> = scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .map(|(host, os)| {
                let channel = &channel;
                let sweep = options.sweep.as_deref();
                scope.spawn(move || Node::prepare(root, host, *os, channel, sweep))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("prepare thread"))
            .collect()
    });
    let mut nodes = Vec::new();
    for ((host, _), node) in hosts.iter().zip(prepared) {
        match node {
            Ok(node) => {
                println!(
                    "fleet: {host} ready, {} jobs, {:.0}s",
                    node.jobs, node.seconds
                );
                nodes.push(node);
            }
            // the others take its share
            Err(error) => println!("fleet: {host} skipped, {error:#}"),
        }
    }
    // the biggest nodes take the test shards
    nodes.sort_by_key(|node| Reverse(node.jobs));

    let local_jobs = available_parallelism().map_or(4, usize::from);
    let mut weights = vec![local_jobs];
    weights.extend(nodes.iter().map(|node| node.jobs));
    let slices = split_scripts(&scripts, weights.len());
    let queue = Queue::new(options.seed, options.cases);
    let queue = &queue;
    // the campaign of every machine runs this build, so it is made once, up front
    let (built, text) = local(
        Command::new("cargo")
            .args(["build", "--release", "-p", "run-rs"])
            .current_dir(root),
    )?;
    if !built {
        bail!("the local build failed:\n{}", last_line(&text));
    }

    let outcomes: Vec<Outcome> = scope(|scope| {
        let mut handles = Vec::new();
        let local_slice = &slices[0];
        handles.push(scope.spawn(move || run_local(root, options, queue, local_jobs, local_slice)));
        if options.tests {
            handles.push(scope.spawn(move || local_tests(root)));
        }
        // a shard goes to a Linux node, a Windows node is the only one of its kind and takes the
        // whole suite
        let mut shards = TEST_SHARDS.iter().copied();
        for (index, node) in nodes.iter().enumerate() {
            let slice = &slices[index + 1];
            let tests = match node.os {
                _ if !options.tests => None,
                Os::Linux => shards.next(),
                Os::Windows => Some(("test suite", "--workspace")),
            };
            handles.push(scope.spawn(move || run_node(root, node, options, queue, slice, tests)));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("worker thread"))
            .collect()
    });

    println!();
    let mut agreed = true;
    for outcome in &outcomes {
        let mark = if outcome.failed { "FAILED" } else { "ok" };
        println!("{} {mark}", outcome.name);
        for line in &outcome.lines {
            println!("  {line}");
        }
        agreed &= !outcome.failed;
    }
    println!(
        "\nfleet: {} in {:.0}s",
        if agreed { "all agree" } else { "FAILED" },
        started.elapsed().as_secs_f64()
    );
    Ok(agreed)
}

fn run_local(
    root: &Path,
    options: &Options,
    queue: &Queue,
    jobs: usize,
    scripts: &[String],
) -> Outcome {
    let mut outcome = Outcome {
        name: "this machine".to_string(),
        ..Outcome::default()
    };
    let interpreter = root.join("target/release/rust");
    let mut tally = Tally::start();
    match current_exe() {
        Ok(exe) => {
            // no `ssh` round trip here, so a small range costs nothing and a busy machine
            // never sits on a big one
            while let Some((seed, cases)) = queue.take(jobs * LOCAL_CASES_PER_JOB) {
                let result = local(
                    Command::new(&exe)
                        .args(["run", "--seed", &seed.to_string()])
                        .args(["--cases", &cases.to_string()])
                        .args(["--timeout-ms", &options.timeout_ms.to_string()])
                        .env("RUSTSCRIPT_INTERPRETER", &interpreter)
                        .current_dir(root),
                );
                tally.add(&mut outcome, result);
            }
        }
        Err(error) => outcome.fail(format!("cannot find the harness binary: {error}")),
    }
    tally.report(&mut outcome);
    if let Some(repo) = &options.sweep
        && !scripts.is_empty()
    {
        let mut bad = 0;
        for script in scripts {
            let checked = local(
                Command::new(&interpreter)
                    .arg("check")
                    .arg(script)
                    .env("RUSTSCRIPT_SKIP_CHECK", "1")
                    .current_dir(repo),
            );
            match checked {
                Ok((true, _)) => {}
                Ok((false, text)) => {
                    bad += 1;
                    outcome.fail(format!("sweep {script}: {}", first_line(&text)));
                }
                Err(error) => {
                    bad += 1;
                    outcome.fail(format!("sweep {script}: {error:#}"));
                }
            }
        }
        outcome.note(format!("sweep: {} scripts, {bad} failed", scripts.len()));
    }
    outcome
}

fn local_tests(root: &Path) -> Outcome {
    let mut outcome = Outcome {
        name: "this machine, test suite".to_string(),
        ..Outcome::default()
    };
    let result = local(
        Command::new("cargo")
            .args(["test", "--workspace", "--no-fail-fast"])
            .current_dir(root),
    );
    test_lines(&mut outcome, "test suite", result);
    outcome
}

fn run_node(
    root: &Path,
    node: &Node,
    options: &Options,
    queue: &Queue,
    scripts: &[String],
    tests: Option<(&str, &str)>,
) -> Outcome {
    let mut outcome = Outcome {
        name: node.host.clone(),
        ..Outcome::default()
    };
    if let Some((label, selection)) = tests {
        let result = node.ssh(&node.in_tree(&format!(
            "cargo test -j {} --no-fail-fast {selection}",
            node.jobs
        )));
        test_lines(&mut outcome, label, result);
    }
    // the cases of an earlier run would look like findings of this one
    if let Err(error) = node.clear_failures() {
        outcome.fail(format!("cannot reach the node: {error:#}"));
        return outcome;
    }
    let mut tally = Tally::start();
    while let Some((seed, cases)) = queue.take(node.jobs * CASES_PER_JOB) {
        let result = node.ssh(&node.in_tree(&format!(
            "{} run --seed {seed} --cases {cases} --timeout-ms {} --jobs {}",
            node.harness(),
            options.timeout_ms,
            node.jobs
        )));
        tally.add(&mut outcome, result);
    }
    tally.report(&mut outcome);
    if tally.findings > 0 {
        match node.fetch_failures(root) {
            Ok(path) => outcome.note(format!("failing cases saved in {}", path.display())),
            Err(error) => outcome.note(format!("cannot fetch the failing cases: {error:#}")),
        }
    }
    if !scripts.is_empty() {
        match node.sweep(scripts) {
            Ok((_, text)) => {
                let bad: Vec<&str> = text.lines().filter(|l| l.starts_with("FAIL ")).collect();
                for line in &bad {
                    outcome.fail(format!("sweep {}", line.trim_start_matches("FAIL ")));
                }
                outcome.note(format!(
                    "sweep: {} scripts, {} failed",
                    scripts.len(),
                    bad.len()
                ));
            }
            Err(error) => outcome.fail(format!("sweep did not run: {error:#}")),
        }
    }
    outcome
}

/// The seeds of the campaign, handed out in small ranges. A machine that is fast or idle comes
/// back for more, so a busy one never holds the run up.
struct Queue {
    seed: u64,
    cases: usize,
    taken: AtomicUsize,
}

impl Queue {
    fn new(seed: u64, cases: usize) -> Queue {
        Queue {
            seed,
            cases,
            taken: AtomicUsize::new(0),
        }
    }

    /// The next range, at most `want` cases, as its first seed and its size.
    fn take(&self, want: usize) -> Option<(u64, usize)> {
        let start = self.taken.fetch_add(want, Ordering::Relaxed);
        if start >= self.cases {
            return None;
        }
        Some((self.seed + start as u64, want.min(self.cases - start)))
    }
}

/// The campaign ranges one machine ran, added up.
struct Tally {
    checked: usize,
    matched: usize,
    findings: usize,
    gaps: usize,
    started: Instant,
}

impl Tally {
    fn start() -> Tally {
        Tally {
            checked: 0,
            matched: 0,
            findings: 0,
            gaps: 0,
            started: Instant::now(),
        }
    }

    /// Reads `checked 400: 398 matched, 2 findings, 0 gaps` and keeps the finding lines.
    fn add(&mut self, outcome: &mut Outcome, result: Result<(bool, String)>) {
        let text = match result {
            Ok((_, text)) => text,
            Err(error) => {
                outcome.fail(format!("campaign did not run: {error:#}"));
                return;
            }
        };
        let Some(summary) = text.lines().find(|line| line.starts_with("checked ")) else {
            outcome.fail(format!("campaign did not run: {}", last_line(&text)));
            return;
        };
        let numbers: Vec<usize> = summary
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        let [checked, matched, findings, gaps, ..] = numbers[..] else {
            outcome.fail(format!("cannot read the campaign summary: {summary}"));
            return;
        };
        self.checked += checked;
        self.matched += matched;
        self.findings += findings;
        self.gaps += gaps;
        // the bucket lines under `findings`, `  SemanticMismatch @ drop N: 2 case(s), seeds ..`
        for line in text
            .lines()
            .skip_while(|line| !line.starts_with("findings"))
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .filter(|line| line.contains("case(s)"))
        {
            outcome.fail(line.trim());
        }
    }

    fn report(&self, outcome: &mut Outcome) {
        let seconds = self.started.elapsed().as_secs_f64();
        let line = format!(
            "campaign: {} cases, {} matched, {} findings, {} gaps, {seconds:.0}s",
            self.checked, self.matched, self.findings, self.gaps
        );
        if self.findings > 0 {
            outcome.fail(line);
        } else {
            outcome.note(line);
        }
    }
}

/// One line when the tests pass, the failed tests when they do not.
fn test_lines(outcome: &mut Outcome, label: &str, result: Result<(bool, String)>) {
    match result {
        Ok((true, text)) => {
            let passed: usize = text
                .lines()
                .filter_map(|line| line.strip_prefix("test result: ok. "))
                .filter_map(|rest| rest.split(' ').next()?.parse::<usize>().ok())
                .sum();
            outcome.note(format!("{label}: {passed} passed"));
        }
        Ok((false, text)) => {
            outcome.fail(format!("{label} failed"));
            for line in text.lines().filter(|line| {
                line.ends_with("FAILED")
                    || line.contains("panicked at")
                    || line.starts_with("error")
            }) {
                outcome.note(line.trim());
            }
        }
        Err(error) => outcome.fail(format!("{label} did not run: {error:#}")),
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// The `channel` of `rust-toolchain.toml`, the version every node builds with.
fn pinned_channel(root: &Path) -> Result<String> {
    let path = root.join("rust-toolchain.toml");
    let text = read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let file: ToolchainFile =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(file.toolchain.channel)
}

/// The Linux and Windows nodes beekeeper reports online. `RUSTSCRIPT_FLEET_NODES` names them by
/// hand, separated by spaces, a Windows node as `name:windows`.
fn fleet_nodes() -> Result<Vec<(String, Os)>> {
    if let Ok(named) = var("RUSTSCRIPT_FLEET_NODES") {
        return Ok(named
            .split_whitespace()
            .map(|name| match name.strip_suffix(":windows") {
                Some(host) => (host.to_string(), Os::Windows),
                None => (name.to_string(), Os::Linux),
            })
            .collect());
    }
    let (ok, text) = local(Command::new("curl").args(["-s", "-m", "10", NODES_URL]))?;
    if !ok {
        bail!("cannot reach beekeeper at {NODES_URL}");
    }
    let nodes: Vec<NodeInfo> =
        serde_json::from_str(&text).context("parse the beekeeper node list")?;
    Ok(nodes
        .into_iter()
        .filter(|node| node.state == "online")
        .filter_map(|node| match node.os.as_str() {
            "linux" => Some((node.hostname, Os::Linux)),
            "windows" => Some((node.hostname, Os::Windows)),
            _ => None,
        })
        .collect())
}

/// The scripts dealt out one by one, so every machine gets a mix of the folders.
fn split_scripts(scripts: &[String], machines: usize) -> Vec<Vec<String>> {
    let mut slices = vec![Vec::new(); machines.max(1)];
    for (index, script) in scripts.iter().enumerate() {
        slices[index % machines.max(1)].push(script.clone());
    }
    slices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_hands_out_every_case_once() {
        let queue = Queue::new(100, 250);
        assert_eq!(queue.take(96), Some((100, 96)));
        assert_eq!(queue.take(96), Some((196, 96)));
        assert_eq!(queue.take(96), Some((292, 58)));
        assert_eq!(queue.take(96), None);
    }

    #[test]
    fn scripts_are_dealt_to_every_machine() {
        let scripts: Vec<String> = (0..7).map(|i| format!("s{i}.rs")).collect();
        let slices = split_scripts(&scripts, 3);
        assert_eq!(slices.iter().map(Vec::len).collect::<Vec<_>>(), [3, 2, 2]);
    }
}
