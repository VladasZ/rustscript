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
    mut report: impl FnMut(ReductionProgress),
) -> Result<(Program, RunResult)> {
    let mut current = original.clone();
    let current_source = current.render();
    let mut current_result = runner.run_source(&current_source)?;
    let mut cache = HashMap::from([(current_source, current_result.clone())]);
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
            let result = if let Some(result) = cache.get(&source) {
                progress.cache_hits += 1;
                result.clone()
            } else {
                let result = runner.run_source(&source)?;
                cache.insert(source, result.clone());
                result
            };
            progress.candidates_checked += 1;
            if result.same_failure(target) {
                progress.reductions_kept += 1;
                report(progress);
                smaller = Some((candidate, result));
                break;
            }
            report(progress);
        }
        let Some((program, result)) = smaller else {
            break;
        };
        current = program;
        current_result = result;
    }
    Ok((current, current_result))
}
