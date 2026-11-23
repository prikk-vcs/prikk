//! Recovery-log runs (A1 item 2): every entry a repair appends shares one run id.

use super::*;

thread_local! {
    /// The run the repair on this thread is writing (A1 item 2): every entry it appends shares this id.
    static CURRENT_RUN: std::cell::Cell<Option<[u8; 8]>> = const { std::cell::Cell::new(None) };
}

/// Counts runs begun by this process, so two runs begun in the same instant still differ.
static RUN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A fresh run id: 8 bytes from a digest of the time, the process and a counter.
pub(super) fn new_run_id() -> [u8; 8] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = RUN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let digest = sha(&[
        nanos.to_be_bytes().as_slice(),
        &std::process::id().to_be_bytes(),
        &count.to_be_bytes(),
    ]
    .concat());
    let mut id = [0_u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

/// Begin a run: every entry appended on this thread until the returned scope ends shares one run id (A1 item 2). A nested call
/// joins the run that is already open, so a repair that calls another repair writes one run.
pub(crate) fn begin_run() -> RunScope {
    let owned = CURRENT_RUN.with(|current| {
        if current.get().is_none() {
            current.set(Some(new_run_id()));
            true
        } else {
            false
        }
    });
    RunScope { owned }
}

/// Ends the run it began, when it was the outermost (see [`begin_run`]).
pub(crate) struct RunScope {
    pub(super) owned: bool,
}

/// 0.51.0 step 1 Part C2 (C2, the review): the open run's own id, hex-encoded the same way
/// [`RecoveryEntryView::id`] is -- so a caller that just saved something under [`begin_run`] can
/// name the run id in its own report, rather than making the user find it in `--recovery-list`.
/// `None` when no run is open (a `--plan-only` preview never calls `begin_run` at all).
pub(crate) fn current_run_id_hex() -> Option<String> {
    CURRENT_RUN.with(|current| current.get().map(|run| prikk_hash::to_hex(&run)))
}

impl Drop for RunScope {
    fn drop(&mut self) {
        if self.owned {
            CURRENT_RUN.with(|current| current.set(None));
            restore::clear_run_kind();
        }
    }
}

/// The run an entry appended now belongs to: the open run, or a run of its own for a save outside any repair.
pub(super) fn run_for_append() -> [u8; 8] {
    CURRENT_RUN.with(|current| match current.get() {
        Some(run) => run,
        None => new_run_id(),
    })
}
