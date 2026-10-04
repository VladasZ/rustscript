//! Runs a check over many items on every core. The example suites start 2 processes per
//! script, and one after another they left all cores but one idle.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{available_parallelism, scope};

/// Every failure message, in item order. A check that panics fails the test through the
/// scope.
pub fn failures<T: Sync>(
    items: &[T],
    check: impl Fn(&T) -> Result<(), String> + Sync,
) -> Vec<String> {
    let next = AtomicUsize::new(0);
    let failed: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    let workers = available_parallelism()
        .map_or(4, usize::from)
        .min(items.len().max(1));
    scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    if let Err(message) = check(item) {
                        failed.lock().expect("failures lock").push((index, message));
                    }
                }
            });
        }
    });
    let mut failed = failed.into_inner().expect("failures lock");
    failed.sort();
    failed.into_iter().map(|(_, message)| message).collect()
}
