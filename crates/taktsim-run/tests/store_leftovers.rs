//! A run directory left in `tmp/` by a process that died is skipped, not an error
//! (ADR-0024 §8). Alone in its test binary: the store numbers its directories in
//! `tmp/` with the process id and a counter of the process, which then starts at 0.

mod support;

use support::{TempDir, drawing_model, run_text};
use taktsim_run::RunRequest;

/// A process that died left `tmp/<pid>-0` to `tmp/<pid>-63`, and this process has
/// its pid, as a process in a container often has: the run skips them and goes
/// through.
#[test]
fn directories_left_in_tmp_by_another_process_are_skipped() {
    let dir = TempDir::new();
    let store = dir.store();
    let tmp = store.root().join("tmp");
    for n in 0..64 {
        std::fs::create_dir_all(tmp.join(format!("{}-{n}", std::process::id()))).unwrap();
    }
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    assert!(response.result.complete);
    assert!(!response.reused);
    // The leftovers are not this process's to remove.
    assert!(tmp.join(format!("{}-0", std::process::id())).is_dir());
}
