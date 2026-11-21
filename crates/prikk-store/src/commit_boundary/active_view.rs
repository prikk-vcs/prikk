//! 0.59.0 RS2c: **one coherent read of an active session's three stores** -- the queue WAL, the ref-name metadata and the commit witness.
//!
//! A `commit` writes them in one order (the metadata, the WAL record, the witness) and a `seal`'s drain in another (the WAL emptied, the witness
//! cleared, the metadata removed). A reader that reads the three at three moments -- `verify` read the WAL at its top, the metadata and the witness
//! stages later, after the whole object scan -- can see a mixture no writer ever left, and RFC 166's classification reads that mixture as damage
//! (`a queued commit you were told had succeeded … is no longer present at all`, `active WAL metadata: missing for non-empty WAL`). **No reading
//! order is right for both writers,** so the three are read **until they are unchanged across one read** (a stat of each before and after, retried a
//! bounded number of times): a view of one moment. Every state a correct writer passes through is one RFC 166 classifies as benign; only a read
//! across moments produces the others. A genuine state that is not changing is read once and reported exactly as before.
//!
//! This takes no lock and writes nothing: three stats, the reads, three stats.

use prikk_error::Result;

use super::active::{ActiveRefMetadata, read_active_ref_metadata_for};
use super::witness::{WitnessState, read_witness};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::wal::{Wal, WalReplay};

/// The reads are repeated at most this many times (after the first) while the stores change under them. A writer that changes them every time the
/// reader looks is a writer running; the last read is then used, as it was before this view existed.
const ATTEMPTS: usize = 24;

/// The default session's WAL, ref-name metadata and witness, read as one moment (see the module doc).
pub(crate) struct ActiveView {
    pub(crate) replay: Result<WalReplay>,
    pub(crate) metadata: Result<ActiveRefMetadata>,
    pub(crate) witness: Result<WitnessState>,
    /// **The exhausted case** (0.59.0 RS2d): the stores still changed on the last of the bounded reads, and the active lock's holder appears to be
    /// running and is not this process -- its pid. The three reads are then not one moment and say nothing a caller may conclude damage from: the
    /// caller reports the publication in flight (the existing finding) instead of classifying them. `None` whenever the read settled, and when it did
    /// not and no other running process holds the active lock (the last read is then used, as it was before this field existed).
    pub(crate) unsettled: Option<u32>,
}

/// What a stat says about one file: its length, modification time and (where the platform has one) identity; `None` when it does not exist.
type Stamp = Option<(u64, Option<std::time::SystemTime>, Option<u64>)>;

fn stamp(path: &std::path::Path) -> Stamp {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = None;
    Some((metadata.len(), metadata.modified().ok(), identity))
}

fn stamps(layout: &RepositoryLayout) -> [Stamp; 3] {
    [
        stamp(&layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)),
        stamp(&layout.active_ref_name_path(DEFAULT_ACTIVE_NAME)),
        stamp(
            &layout
                .active_session_dir(DEFAULT_ACTIVE_NAME)
                .join("witness"),
        ),
    ]
}

/// Read the default session's three stores until a read leaves them unchanged (or the attempts run out).
pub(crate) fn read_active_view(layout: &RepositoryLayout) -> ActiveView {
    let mut attempts_left = ATTEMPTS;
    loop {
        let before = stamps(layout);
        let mut view = ActiveView {
            replay: Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay(),
            metadata: read_active_ref_metadata_for(layout, DEFAULT_ACTIVE_NAME),
            witness: read_witness(layout, DEFAULT_ACTIVE_NAME),
            unsettled: None,
        };
        #[cfg(test)]
        crate::refs::view_hooks::run("active-view-after-read");
        if stamps(layout) == before {
            return view;
        }
        if attempts_left == 0 {
            // Exhausted: the stores are still changing. A holder of the active lock that appears to be running (and is not this process) is the
            // writer doing it: in flight. Otherwise this is the last read, as before.
            view.unsettled = crate::refs::in_flight::active_publisher(layout)
                .ok()
                .flatten();
            return view;
        }
        attempts_left -= 1;
    }
}

/// What a reader saw of the **active lock** at one moment: the pid of another running process that holds it, and the lock file's stamp (0.59.0 RS2e).
/// Two of these, taken either side of a stage, say whether a writer was running during it: a holder at either, or a lock file that appeared, changed or
/// went away between them (a writer that started and finished inside the stage leaves exactly that).
pub(crate) struct LockWatch {
    pid: Option<u32>,
    stamp: Stamp,
}

impl LockWatch {
    /// Look at the default session's active lock now.
    pub(crate) fn take(layout: &RepositoryLayout) -> Self {
        Self {
            pid: crate::refs::in_flight::active_publisher(layout)
                .ok()
                .flatten(),
            stamp: stamp(&layout.active_lock_path(DEFAULT_ACTIVE_NAME)),
        }
    }

    /// Whether a writer was running between `self` and `later`: `Some(Some(pid))` when a holder was seen, `Some(None)` when only the lock file's
    /// change says so, `None` when neither does.
    pub(crate) fn writer_until(&self, later: &Self) -> Option<Option<u32>> {
        if let Some(pid) = later.pid.or(self.pid) {
            return Some(Some(pid));
        }
        (self.stamp != later.stamp).then_some(None)
    }
}
