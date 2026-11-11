//! Cancellation.
//!
//! A cooperative flag for the managed engines, plus the process id so a
//! command-line engine can be killed outright: waiting for `7z` to finish a
//! 40 GB archive after the user pressed cancel is not cancellation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Every long loop asks this; `None` means "cannot be cancelled" (a CLI pipe).
pub type Cancel<'a> = Option<&'a CancelToken>;

pub fn stopped(cancel: Cancel<'_>) -> bool {
    cancel.map(CancelToken::cancelled).unwrap_or(false)
}
