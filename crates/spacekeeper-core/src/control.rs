//! Cooperative cancellation and pause/resume.
//!
//! A scan touches millions of paths across a Rayon pool, blocking `walkdir`
//! iterators and async scanners that shell out to Docker. Rather than have two
//! mechanisms, [`Control`] is a single cheap handle that works from both worlds:
//! synchronous workers block on a condvar, async tasks poll it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::error::{Error, Result};

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    paused: Mutex<bool>,
    resumed: Condvar,
}

/// Clonable handle used to stop or pause a running operation.
///
/// Cancellation is one-way: once cancelled, a handle stays cancelled. A new
/// scan gets a new handle, which removes a whole class of "stale stop button"
/// races.
#[derive(Debug, Clone, Default)]
pub struct Control {
    inner: Arc<Inner>,
}

impl Control {
    /// Create a fresh, running handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Also wakes anything blocked on a pause.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        // A paused worker must be able to observe the cancellation.
        self.set_paused(false);
        self.inner.resumed.notify_all();
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Pause the operation. Has no effect once cancelled.
    pub fn pause(&self) {
        if !self.is_cancelled() {
            self.set_paused(true);
        }
    }

    /// Resume a paused operation.
    pub fn resume(&self) {
        self.set_paused(false);
        self.inner.resumed.notify_all();
    }

    /// Whether the operation is currently paused.
    pub fn is_paused(&self) -> bool {
        *self.lock_paused()
    }

    /// Checkpoint for synchronous workers.
    ///
    /// Returns [`Error::Cancelled`] if the user stopped the operation, and
    /// blocks (without spinning) for as long as it is paused.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.block_while_paused();
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    /// Checkpoint for async tasks. Yields to the runtime instead of blocking a
    /// worker thread while paused.
    pub async fn check_async(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        while self.is_paused() && !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    fn block_while_paused(&self) {
        let mut paused = self.lock_paused();
        while *paused && !self.is_cancelled() {
            // Timed wait so a missed notification can never wedge a worker.
            let (guard, _) = self
                .inner
                .resumed
                .wait_timeout(paused, Duration::from_millis(200))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            paused = guard;
        }
    }

    fn set_paused(&self, value: bool) {
        *self.lock_paused() = value;
    }

    /// Lock the pause flag, recovering from poisoning.
    ///
    /// A panicking worker must not permanently wedge every other worker, and
    /// the protected value is a single `bool` that cannot be left inconsistent.
    fn lock_paused(&self) -> std::sync::MutexGuard<'_, bool> {
        self.inner
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_handle_is_running() {
        let control = Control::new();
        assert!(!control.is_cancelled());
        assert!(!control.is_paused());
        assert!(control.check().is_ok());
    }

    #[test]
    fn cancellation_is_observed_by_check() {
        let control = Control::new();
        control.cancel();
        assert!(control.is_cancelled());
        assert!(matches!(control.check(), Err(Error::Cancelled)));
    }

    #[test]
    fn cancellation_is_shared_between_clones() {
        let control = Control::new();
        let clone = control.clone();
        clone.cancel();
        assert!(control.is_cancelled());
    }

    #[test]
    fn pause_then_resume_unblocks_a_worker() {
        let control = Control::new();
        control.pause();
        assert!(control.is_paused());

        let worker = {
            let control = control.clone();
            std::thread::spawn(move || control.check())
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!worker.is_finished(), "worker should still be parked");

        control.resume();
        assert!(worker.join().expect("worker panicked").is_ok());
    }

    #[test]
    fn cancelling_releases_a_paused_worker() {
        let control = Control::new();
        control.pause();
        let worker = {
            let control = control.clone();
            std::thread::spawn(move || control.check())
        };
        std::thread::sleep(Duration::from_millis(50));
        control.cancel();
        assert!(matches!(
            worker.join().expect("worker panicked"),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn pause_is_ignored_after_cancel() {
        let control = Control::new();
        control.cancel();
        control.pause();
        assert!(
            !control.is_paused(),
            "a cancelled scan must not be pausable"
        );
    }
}
