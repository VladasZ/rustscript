//! Runs a check over many items on every core. A case costs a `rustc` run and 3 process
//! starts, and one after another they left all cores but one idle.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{available_parallelism, scope};

/// The result of every item, in item order.
pub fn map<T: Sync, R: Send>(items: &[T], run: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::new());
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
                    let result = run(item);
                    done.lock().expect("results lock").push((index, result));
                }
            });
        }
    });
    let mut done = done.into_inner().expect("results lock");
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}
