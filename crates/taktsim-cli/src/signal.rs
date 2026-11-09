//! The one flag a signal sets (ADR-0025 §9). Only this lives in the binary, so the
//! Python bindings can set the same flag: SIGINT, SIGTERM, Ctrl+C and Ctrl+Break cut
//! the running replications at once, and only finished ones count.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set once a signal arrives.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Installs the handler and returns its flag, or why it could not.
pub(crate) fn install() -> Result<&'static AtomicBool, ctrlc::Error> {
    ctrlc::set_handler(|| CANCEL.store(true, Ordering::Relaxed))?;
    Ok(&CANCEL)
}
