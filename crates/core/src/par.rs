//! Running work on a few threads.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Applies `f` to every item using up to `jobs` threads and returns the results in order.
pub fn map<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, items.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    *results[i].lock().expect("no panics while holding the lock") = Some(r);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|m| {
            m.into_inner()
                .expect("no poisoned locks")
                .expect("every item was processed")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_order_and_handles_edge_cases() {
        let items: Vec<u32> = (0..100).collect();
        assert_eq!(
            super::map(&items, 8, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        assert!(super::map(&[] as &[u32], 8, |x| *x).is_empty());
        assert_eq!(super::map(&[1], 0, |x| x + 1), [2]);
    }
}
