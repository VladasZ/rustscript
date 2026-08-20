use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::model::Program;
use crate::runner::{RunResult, Runner};

#[derive(Clone, Copy, Debug, Default)]
pub struct ReductionProgress {
    pub candidates_checked: usize,
    pub reductions_kept: usize,
    pub cache_hits: usize,
}

pub fn reduce(
    runner: &Runner,
    original: &Program,
    target: &RunResult,
) -> Result<(Program, RunResult)> {
    reduce_with_progress(runner, original, target, |_| {})
}

pub fn reduce_with_progress(
    runner: &Runner,
    original: &Program,
    target: &RunResult,
    report: impl FnMut(ReductionProgress),
) -> Result<(Program, RunResult)> {
    reduce_by(|source| runner.run_source(source), original, target, report)
}

/// Greedy descent over `shrink_candidates`, with `run` as the oracle. A
/// candidate is taken only when its rendering is strictly shorter than the
/// current one. A shrink can rewrite a node into a form of the same size
/// whose own shrinks lead back to the original, and accepting every
/// reproducing candidate then cycles between them forever.
pub fn reduce_by(
    mut run: impl FnMut(&str) -> Result<RunResult>,
    original: &Program,
    target: &RunResult,
    mut report: impl FnMut(ReductionProgress),
) -> Result<(Program, RunResult)> {
    let mut current = original.clone();
    let mut current_source = current.render();
    let mut current_result = run(&current_source)?;
    let mut cache = HashMap::from([(current_source.clone(), current_result.clone())]);
    let mut progress = ReductionProgress::default();
    if !current_result.same_failure(target) {
        bail!(
            "program model produced {:?}, expected {:?}",
            current_result.classification,
            target.classification
        );
    }
    loop {
        let mut smaller = None;
        for candidate in current.shrink_candidates() {
            let source = candidate.render();
            if source.len() >= current_source.len() {
                continue;
            }
            let result = if let Some(result) = cache.get(&source) {
                progress.cache_hits += 1;
                result.clone()
            } else {
                let result = run(&source)?;
                cache.insert(source.clone(), result.clone());
                result
            };
            progress.candidates_checked += 1;
            if result.same_failure(target) {
                progress.reductions_kept += 1;
                report(progress);
                smaller = Some((candidate, source, result));
                break;
            }
            report(progress);
        }
        let Some((program, source, result)) = smaller else {
            break;
        };
        current = program;
        current_source = source;
        current_result = result;
    }
    Ok((current, current_result))
}
