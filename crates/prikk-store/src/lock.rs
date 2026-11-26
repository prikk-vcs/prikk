//! Simple file locks for active-session and ref writers.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    DirectoryHold, MutationRoot, create_lock_file_required, inspect_entry, read_file_if_exists,
    remove_lock_file_best_effort,
};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};

/// Active session lock acquired before mutating an active WAL tail.
#[derive(Debug)]
pub struct ActiveLock {
    path: PathBuf,
    relative: PathBuf,
    mutation_root: MutationRoot,
    /// Declared last, so it is released after `Drop::drop` has removed the lock file through the held directories (0.58.0 P2a), and
    /// so the held record (0.59.0 P5b3b) names the lock until its file is gone.
    _hold: LockHold,
}

impl ActiveLock {
    /// Acquire a lock through exclusive file creation, for the active session named `name` (RFC
    /// 108 increment 1: previously hardcoded to `"default"`; generalizes the same way
    /// `RefLock::acquire` below already takes `ref_name`).
    ///
    /// RFC 108 increment 3a: `name` widened from `&str` to `impl AsRef<Path>`, matching
    /// `wal.rs::Wal::for_layout`. No other change needed here -- `relative` was already **derived**
    /// from `path` via `repository_relative`, never reconstructed from `name` the way `Wal` used to.
    ///
    /// RFC 155 Part I-A2: refuses while an archive import's journal is present, for **every**
    /// session name, not only the default one (corrected from Part I-A's own first cut, which
    /// checked `is_default` here -- review `rfc155-impl-IA-review-v1`, finding IA1(b): RFC 108's
    /// other sessions can append objects too, and a cancel would otherwise cut their work out from
    /// under them with no refusal having warned them off first). Confirmed from source that nothing
    /// relies on the old, narrower behavior: every production writer that takes this lock --
    /// `commit`, `seal`, `trust maintainer add`, `bundle import`, and the rest -- passes
    /// `DEFAULT_ACTIVE_NAME` unconditionally; the only non-default callers anywhere in the
    /// workspace are RFC 108's own multi-session tests, none of which runs while an import journal
    /// is present. This is the structural half of §9.5's rule ("every repository writer refuses"):
    /// every caller of this function, present and future, is covered with no per-call-site
    /// enumeration to remember, because this is -- by this module's own established doc note on
    /// `ensure_write_state` -- "the one call site every writer passes through."
    /// `Self::acquire_allowing_import_journal` is the named, narrow exception, used only by the
    /// import itself (so a resume can re-acquire this lock while its own journal is present), the
    /// cancel, and doctor's own recovery verbs.
    pub fn acquire(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Result<Self> {
        Self::acquire_checked(layout, name, true)
    }

    /// As [`Self::acquire`], but never refuses for an archive import's own journal -- the named
    /// exception [`Self::acquire`]'s own doc comment describes. Used only by: `archive::import`'s
    /// own lock acquisition (a resume must be able to re-acquire this lock while its own journal is
    /// still present), `archive::import::cancel_import`, and doctor's recovery verbs
    /// (`repair_tails`/`plan_repair_tails`, `discard_damaged_commits`, `restore_queue_target`,
    /// `repair_repository`'s per-active loop, and `recovery_restore`/`recovery_clear` via
    /// `recovery_log::repair_locks`) -- enumerated here, from source, the same way RFC 163
    /// enumerated its own write-side tail-guard call sites, so this doc comment's own list is a
    /// checkable claim, not a label.
    pub(crate) fn acquire_allowing_import_journal(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::acquire_checked(layout, name, false)
    }

    fn acquire_checked(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
        check_import_journal: bool,
    ) -> Result<Self> {
        if check_import_journal {
            crate::foundation::import_journal_marker::refuse_while_present(layout)?;
        }
        let path = layout.active_lock_path(name);
        let relative = layout.repository_relative(&path)?;
        let mutation_root = layout.repository_mutation_root().clone();
        let hold = acquire_lock_file(&mutation_root, &relative, &path, "active", Rank::Active)?;
        Ok(Self {
            path,
            relative,
            mutation_root,
            _hold: hold,
        })
    }

    /// The lock a writer takes. RFC 168 §3.1, §3.3: a repository created before the recovery log and the witness gets them here,
    /// at its first write. A plan-only path takes [`Self::acquire`] instead and creates nothing (RFC 168 F6). Created after the lock
    /// is held and returned as the guard, so a failure here releases the lock on drop.
    pub fn acquire_for_write(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Result<Self> {
        let is_default = name.as_ref() == Path::new(crate::foundation::layout::DEFAULT_ACTIVE_NAME);
        let lock = Self::acquire(layout, name)?;
        if is_default {
            layout.ensure_write_state()?;
        }
        Ok(lock)
    }

    /// As [`Self::acquire_for_write`], built on [`Self::acquire_allowing_import_journal`] instead of
    /// [`Self::acquire`] -- doctor's own recovery verbs that also need `ensure_write_state` run
    /// (`repair_tails`, `repair_repository`'s per-active loop) use this one.
    pub(crate) fn acquire_for_write_allowing_import_journal(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
    ) -> Result<Self> {
        let is_default = name.as_ref() == Path::new(crate::foundation::layout::DEFAULT_ACTIVE_NAME);
        let lock = Self::acquire_allowing_import_journal(layout, name)?;
        if is_default {
            layout.ensure_write_state()?;
        }
        Ok(lock)
    }

    /// Return lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn require_layout(&self, layout: &RepositoryLayout) -> Result<()> {
        if self
            .mutation_root
            .same_authority(layout.repository_mutation_root())
        {
            return Ok(());
        }
        Err(PrikkError::LockConflict(
            "active lock belongs to a different repository authority".to_string(),
        ))
    }
}

impl Drop for ActiveLock {
    fn drop(&mut self) {
        remove_lock_file_best_effort(&self.mutation_root, &self.relative);
    }
}

/// Ref-specific lock acquired before publishing one ref pointer.
#[derive(Debug)]
pub struct RefLock {
    path: PathBuf,
    relative: PathBuf,
    mutation_root: MutationRoot,
    /// Declared last, so it is released after `Drop::drop` has removed the lock file through the held directories (0.58.0 P2a), and
    /// so the held record (0.59.0 P5b3b) names the lock until its file is gone.
    _hold: LockHold,
}

impl RefLock {
    /// Acquire a ref lock through exclusive file creation.
    pub fn acquire(layout: &RepositoryLayout, ref_name: &str) -> Result<Self> {
        let path = layout.ref_lock_path(ref_name);
        let relative = layout.repository_relative(&path)?;
        let mutation_root = layout.repository_mutation_root().clone();
        let hold = acquire_lock_file(&mutation_root, &relative, &path, "ref", Rank::Ref)?;
        Ok(Self {
            path,
            relative,
            mutation_root,
            _hold: hold,
        })
    }

    /// Return lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RefLock {
    fn drop(&mut self) {
        remove_lock_file_best_effort(&self.mutation_root, &self.relative);
    }
}

/// One held lock on a `LockableContainer`. Never constructed directly outside
/// `acquire_container_locks` -- the sorted-order guarantee that helper provides is only real if
/// nothing can acquire a container lock any other way.
#[derive(Debug)]
struct ContainerLockHandle {
    /// RFC 155 Part I-A2: which container this handle actually holds -- read back by
    /// [`ContainerLockGuard::holds`], so a caller that needs to *prove* a lock is held (rather
    /// than simply trusting its own bookkeeping) has something to check against.
    kind: LockableContainer,
    relative: PathBuf,
    mutation_root: MutationRoot,
    /// Declared last, so it is released after `Drop::drop` has removed the lock file through the held directories (0.58.0 P2a), and
    /// so the held record (0.59.0 P5b3b) names the lock until its file is gone.
    _hold: LockHold,
}

impl Drop for ContainerLockHandle {
    fn drop(&mut self) {
        remove_lock_file_best_effort(&self.mutation_root, &self.relative);
    }
}

/// RAII guard for one or more container locks, acquired together by `acquire_container_locks` and
/// released when dropped. Held for its `Drop` effect, not read from -- the same shape
/// `ActiveLock`/`RefLock` already use.
#[derive(Debug)]
pub struct ContainerLockGuard {
    handles: Vec<ContainerLockHandle>,
}

impl ContainerLockGuard {
    /// Whether this guard actually holds `container`'s own lock -- RFC 155 Part I-A2's own need:
    /// `object_store`'s under-held-lock append path (import's stage 1, which holds `ObjectStore`
    /// for its whole run rather than once per append) checks this before appending, rather than
    /// trusting a caller's claim that it already holds the lock it is about to rely on.
    pub(crate) fn holds(&self, container: LockableContainer) -> bool {
        self.handles.iter().any(|handle| handle.kind == container)
    }
}

/// Acquire every lock in `containers`, sorted into `LockableContainer`'s fixed `Ord` before any file
/// is created (design-v1.md §15.7's deadlock ruling: a single acquisition helper that sorts the
/// caller's requested set, not per-call-site ordering discipline) -- so two call sites that each
/// request `{RefPointerIndex, RefLog}` always acquire them in the same order regardless of which order
/// their own arguments list them in.
///
/// If any acquisition in the sorted sequence fails -- most commonly `LockConflict`, a concurrent
/// writer or the compactor already holding a later container in the order -- every lock already
/// acquired during this call is released before the error returns: `handles` (built incrementally,
/// `?`-propagating on failure) simply drops here, and each already-acquired `ContainerLockHandle`'s
/// own `Drop` releases it. A partial, leaked hold on early-failure is exactly the wedge shape this
/// stage's stale-lock recovery work exists to stop introducing more of, so this path must never leave
/// one behind.
///
/// RFC 155 Part I-B2: refuses while an archive import's journal is present, mirroring
/// [`ActiveLock::acquire`]'s own check (review `rfc155-impl-IB-review-v1`, finding IB1(b)) --
/// `ActiveLock::acquire` alone was not "the one call site every writer passes through" after all:
/// `compact --pointer-index`/`--received-index`/`--trust-policy`, `doctor --rebuild-pointer-index`,
/// and `doctor --repair-index`/`--repair-pointer-index-tail` each take only a container lock, with
/// no `ActiveLock` at all, so none of them passed through Part I-A2's own fix. [`acquire_container_locks`]
/// is the one function every one of those, and every ordinary writer that also holds `ActiveLock`
/// first, passes through -- so this is now the actual "one call site," not `ActiveLock::acquire`
/// alone. `acquire_container_locks_allowing_import_journal` is the named, narrow exception, used
/// only by the import itself, the cancel, and doctor's own recovery verbs (`repair_tails`/
/// `plan_repair_tails`, `recovery_log::repair_locks` backing `--recovery-restore`/`--recovery-clear`)
/// -- enumerated here, from source, the same way `ActiveLock::acquire_allowing_import_journal`'s own
/// doc comment does for its list. `doctor --repair-index`/`--repair-pointer-index-tail` are
/// deliberately **not** exempted: unlike `repair_tails`/`--recovery-restore`/`--recovery-clear`,
/// neither is part of the import's own resume/cancel flow (the interrupted-import issue only ever
/// names resume or `--cancel-import`), and rebuilding the object index or truncating a pointer-index
/// tail mid-import is exactly the "another writer mutates a journaled container while the journal
/// still names its old length" hazard this part exists to close -- the same reasoning that keeps
/// `compact`/`--rebuild-pointer-index` refusing too.
pub fn acquire_container_locks(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
) -> Result<ContainerLockGuard> {
    acquire_container_locks_checked(layout, containers, true)
}

/// As [`acquire_container_locks`], but never refuses for an archive import's own journal -- the
/// named exception that function's own doc comment describes. Used only by: `archive::import`'s own
/// lock acquisition (a resume must be able to re-acquire these locks while its own journal is still
/// present), `archive::import::cancel_import`, `doctor::repair_tails`/`plan_repair_tails`, and
/// `recovery_log::repair_locks` (backing `--recovery-restore`/`--recovery-clear`).
pub(crate) fn acquire_container_locks_allowing_import_journal(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
) -> Result<ContainerLockGuard> {
    acquire_container_locks_checked(layout, containers, false)
}

fn acquire_container_locks_checked(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
    check_import_journal: bool,
) -> Result<ContainerLockGuard> {
    if check_import_journal {
        crate::foundation::import_journal_marker::refuse_while_present(layout)?;
    }
    let mut sorted = containers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mutation_root = layout.repository_mutation_root().clone();
    let mut handles = Vec::with_capacity(sorted.len());
    for container in sorted {
        let path = layout.lockable_container_lock_path(container);
        let relative = layout.repository_relative(&path)?;
        let hold = acquire_lock_file(
            &mutation_root,
            &relative,
            &path,
            container_lock_kind(container),
            Rank::Container(container),
        )?;
        handles.push(ContainerLockHandle {
            kind: container,
            relative,
            mutation_root: mutation_root.clone(),
            _hold: hold,
        });
    }
    Ok(ContainerLockGuard { handles })
}

fn container_lock_kind(container: LockableContainer) -> &'static str {
    match container {
        LockableContainer::RefPointerIndex => "container:ref-pointer-index",
        LockableContainer::RefLog => "container:ref-log",
        LockableContainer::ReceivedIndex => "container:received-index",
        LockableContainer::TrustPolicy => "container:trust-policy",
        LockableContainer::ObjectStore => "container:object-store",
    }
}

/// The one function every lock guard takes its lock file through, so it is also the one place a guard registers its hold on the
/// root's held directories (0.58.0 P2a) **and in the held record** (0.59.0 P5b3b): it returns the [`LockHold`], and a guard cannot be
/// built without storing one. The hold is registered before the file is created, so the creation's own walk is already kept; on a
/// refusal it is released here. The held record names the lock only once the file exists, and until the guard has removed it.
///
/// A create that finds the name taken waits for it when, and only when, [`wait_decision`] says to (a holder that appears to be running,
/// a lock that ranks after everything this thread holds, the bound not reached); every other answer is today's refusal, at once.
fn acquire_lock_file(
    mutation_root: &MutationRoot,
    relative: &Path,
    path: &Path,
    kind: &str,
    rank: Rank,
) -> Result<LockHold> {
    let hold = mutation_root.hold_directories();
    let body = lock_body(kind);
    let mut waiting = Waiting::new();
    loop {
        match create_lock_file(mutation_root, relative, body.as_bytes()) {
            Ok(()) => {
                return Ok(LockHold {
                    _held: HeldToken::register(rank, path),
                    _hold: hold,
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                match wait_decision(mutation_root, relative, path, kind, rank, &mut waiting) {
                    Decision::Retry => {}
                    Decision::Refuse(suffix) => {
                        return Err(PrikkError::LockConflict(format!(
                            "{kind} lock already exists: {} -- run `prikk unlock` to list held locks, or \
                             `prikk unlock --lock <path>` to clear one once the process that held it is \
                             confirmed gone{suffix}",
                            path.display()
                        )));
                    }
                }
            }
            // `create_lock_file_required` puts the file's path into every failure but "already exists".
            Err(err) => {
                return Err(PrikkError::Io {
                    kind: Some(err.kind()),
                    context: format!("{kind} lock: {err}"),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------------------
// 0.59.0 P5b3b: one waiting policy for a holder that is running.
//
// Every lock is taken by an exclusive create, and a create that finds the name taken used to refuse at once. It now waits, for a bounded
// time, when the holder **appears to be running** (the liveness judge of `lock_holder`), and for nothing else: a holder that does not appear
// to be running, one that cannot be judged, a body that stays incomplete, a lock this process itself holds, and a lock that does not rank after
// everything this thread already holds are all refused at once, with today's sentence. Waiting writes nothing: no file, no state.
//
// **Why two waiters cannot wait on each other.** The locks have one fixed order: the active lock, then the ref lock, then the container locks
// in `LockableContainer`'s declaration order (the object store last). A thread waits only for a lock that ranks **after every lock it
// holds**, read from the held record below, so every wait-for edge goes up the order and there is no cycle. This is not decorative: `archive
// import` takes its container locks and *then* the active lock, the one inversion in the code; without the rule a `sync seal` (holds the
// active lock, wants the object store) and an import (holds the object store, wants the active lock) would wait for each other to the bound.
// The rule holds because the record is complete: **every lock kind is registered here, in `acquire_lock_file`, and unregistered when its
// guard drops**, and nothing else acquires one.
// ---------------------------------------------------------------------------------------------------------------------------------------------

/// Where a lock stands in the one order. The derived order is the declaration order: active, ref, then the containers as
/// [`LockableContainer`] orders them (the object store last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank {
    /// The active-session lock.
    Active,
    /// A ref lock (every ref's lock has this one rank: two are never waited for in turn).
    Ref,
    /// One container's lock.
    Container(LockableContainer),
}

/// One lock this process holds, as the held record keeps it.
#[derive(Debug)]
struct Held {
    id: u64,
    thread: ThreadId,
    rank: Rank,
    path: PathBuf,
}

/// The locks held right now, by every thread of this process. A thread's rank rule reads its own; "this process holds it" reads all.
static HELD: Mutex<Vec<Held>> = Mutex::new(Vec::new());
static NEXT_HELD: AtomicU64 = AtomicU64::new(1);

fn with_held<T>(read: impl FnOnce(&mut Vec<Held>) -> T) -> T {
    let mut held = HELD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    read(&mut held)
}

/// A lock's entry in the held record, removed when the guard that owns it drops (from any thread).
#[derive(Debug)]
struct HeldToken(u64);

impl HeldToken {
    fn register(rank: Rank, path: &Path) -> Self {
        let id = NEXT_HELD.fetch_add(1, Ordering::Relaxed);
        with_held(|held| {
            held.push(Held {
                id,
                thread: std::thread::current().id(),
                rank,
                path: path.to_path_buf(),
            });
        });
        Self(id)
    }
}

impl Drop for HeldToken {
    fn drop(&mut self) {
        with_held(|held| held.retain(|entry| entry.id != self.0));
    }
}

/// What a guard keeps of its acquisition: the held record's entry, then the held directories. Field order is drop order.
#[derive(Debug)]
struct LockHold {
    _held: HeldToken,
    _hold: DirectoryHold,
}

/// Whether this thread holds a lock that ranks at or after `rank` (so a wait for `rank` could meet a wait for what this thread holds).
pub(crate) fn this_thread_holds_at_or_after(rank: Rank) -> bool {
    let me = std::thread::current().id();
    with_held(|held| {
        held.iter()
            .any(|entry| entry.thread == me && entry.rank >= rank)
    })
}

fn this_process_holds(path: &Path) -> bool {
    with_held(|held| held.iter().any(|entry| entry.path == path))
}

/// The ranks this thread holds, for the tests that read the record.
#[cfg(test)]
pub(crate) fn held_ranks_of_this_thread() -> Vec<Rank> {
    let me = std::thread::current().id();
    with_held(|held| {
        let mut ranks: Vec<Rank> = held
            .iter()
            .filter(|entry| entry.thread == me)
            .map(|entry| entry.rank)
            .collect();
        ranks.sort_unstable();
        ranks
    })
}

/// The wait's default, in seconds, and the ceiling `PRIKK_LOCK_WAIT_SECONDS` is held to.
pub const DEFAULT_LOCK_WAIT_SECONDS: u64 = 5;
/// The longest a command can be told to wait for one stop (five minutes).
pub const MAX_LOCK_WAIT_SECONDS: u64 = 300;
/// The environment variable that sets the bound, in whole seconds. `0` is the old behaviour: refuse at once.
pub const LOCK_WAIT_ENV: &str = "PRIKK_LOCK_WAIT_SECONDS";

/// How long a stop may wait, from the variable's text: absent or not a whole number of seconds is the default, `0` is off, and anything
/// above the ceiling is the ceiling.
#[must_use]
pub fn parse_lock_wait(value: Option<&str>) -> Duration {
    let seconds = match value.map(str::trim) {
        None => DEFAULT_LOCK_WAIT_SECONDS,
        Some(text) => text
            .parse::<u64>()
            .map_or(DEFAULT_LOCK_WAIT_SECONDS, |n| n.min(MAX_LOCK_WAIT_SECONDS)),
    };
    Duration::from_secs(seconds)
}

/// Whether this process may wait at all. A command that reports or repairs (`verify`, `status`, `doctor`, `unlock`, `compact`) turns it off
/// for its whole run: such a command refuses at once, as it always did.
static WAITING_ALLOWED: AtomicBool = AtomicBool::new(true);

/// This process will never wait for a lock or for a publication in flight: every stop is refused at once, as before 0.59.0. Called once, by
/// the commands that report or repair.
pub fn refuse_without_waiting() {
    WAITING_ALLOWED.store(false, Ordering::Relaxed);
}

#[cfg(test)]
thread_local! {
    /// A test's own bound. The unit tests of every other module hold locks and expect to be refused at once, so under `cfg(test)` the bound is
    /// off unless the test sets one.
    static TEST_BOUND: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

/// Run `body` with the wait set to `bound` on this thread (tests only).
#[cfg(test)]
pub(crate) fn with_lock_wait<T>(bound: Duration, body: impl FnOnce() -> T) -> T {
    let before = TEST_BOUND.with(|cell| cell.replace(Some(bound)));
    let out = body();
    TEST_BOUND.with(|cell| cell.set(before));
    out
}

/// The bound of this stop.
pub(crate) fn lock_wait_bound() -> Duration {
    if !WAITING_ALLOWED.load(Ordering::Relaxed) {
        return Duration::ZERO;
    }
    #[cfg(test)]
    {
        TEST_BOUND
            .with(std::cell::Cell::get)
            .unwrap_or(Duration::ZERO)
    }
    #[cfg(not(test))]
    {
        parse_lock_wait(std::env::var(LOCK_WAIT_ENV).ok().as_deref())
    }
}

/// How often a waiter looks again.
const POLL: Duration = Duration::from_millis(2);
/// How long a body that is still empty or short is given before it is called incomplete (0.59.0 P5b3c: 250 ms; P5b3b had 30). It is spent only on a
/// lock whose body is not there yet: a creator between its create and its write, or one killed there. A live creator writes it within a few
/// milliseconds on an idle machine; the grace is wide enough for a loaded one, and a killed creator's lock is refused this much later.
const INCOMPLETE_GRACE: Duration = Duration::from_millis(250);

/// The grace in force: [`INCOMPLETE_GRACE`], or, in a test that has set one, that.
fn incomplete_grace() -> Duration {
    #[cfg(test)]
    if let Some(grace) = test_hooks::GRACE.with(std::cell::Cell::get) {
        return grace;
    }
    INCOMPLETE_GRACE
}

/// What the tests of the grace see and set (`cfg(test)` only). No test of the grace depends on a thread meeting a deadline: a test sets the grace,
/// and a writer thread waits for the waiter's look at the empty body (`looks`) instead of for the clock.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    thread_local! {
        pub(super) static GRACE: Cell<Option<Duration>> = const { Cell::new(None) };
        static LOOKS: RefCell<Option<Arc<AtomicUsize>>> = const { RefCell::new(None) };
        static TRACE: RefCell<(Option<Instant>, Vec<String>)> = const { RefCell::new((None, Vec::new())) };
    }

    /// Run `body` with the grace set to `grace` on this thread; every look at an incomplete body is counted in `looks`.
    pub(crate) fn with_incomplete_grace<T>(
        grace: Duration,
        looks: Arc<AtomicUsize>,
        body: impl FnOnce() -> T,
    ) -> T {
        let before = GRACE.with(|cell| cell.replace(Some(grace)));
        LOOKS.with(|cell| *cell.borrow_mut() = Some(looks));
        TRACE.with(|cell| *cell.borrow_mut() = (None, Vec::new()));
        let out = body();
        LOOKS.with(|cell| *cell.borrow_mut() = None);
        GRACE.with(|cell| cell.set(before));
        out
    }

    pub(super) fn look_at_an_incomplete_body() {
        LOOKS.with(|cell| {
            if let Some(looks) = cell.borrow().as_ref() {
                looks.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    /// Record what the judge said at a look (the failure message of a test prints these: what was read, and when).
    pub(super) fn note(what: String) {
        TRACE.with(|cell| {
            let mut cell = cell.borrow_mut();
            let first = *cell.0.get_or_insert_with(Instant::now);
            cell.1
                .push(format!("+{} ms: {what}", first.elapsed().as_millis()));
        });
    }

    /// The looks of this thread's last wait, for an assertion's message.
    pub(crate) fn trace() -> String {
        TRACE.with(|cell| cell.borrow().1.join("; "))
    }
}
/// A waiter says so on stderr once it has waited this long.
const ANNOUNCE_AFTER: Duration = Duration::from_secs(1);

/// What one stop has done so far.
pub(crate) struct Waiting {
    started: Option<Instant>,
    announced: bool,
    incomplete_since: Option<Instant>,
    pid: Option<u32>,
}

impl Waiting {
    pub(crate) fn new() -> Self {
        Self {
            started: None,
            announced: false,
            incomplete_since: None,
            pid: None,
        }
    }

    /// Time spent waiting so far, `None` before the first wait.
    fn waited(&self) -> Option<Duration> {
        self.started.map(|started| started.elapsed())
    }

    /// The one stderr line, once, after a second: `what` names what is waited for.
    pub(crate) fn announce_if_due(&mut self, bound: Duration, what: impl FnOnce() -> String) {
        if self.announced {
            return;
        }
        if self.waited().is_some_and(|waited| waited >= ANNOUNCE_AFTER) {
            self.announced = true;
            eprintln!("waiting for {}; giving up after {}", what(), seconds(bound));
        }
    }

    /// Start the clock at the first wait.
    pub(crate) fn begin(&mut self) {
        self.started.get_or_insert_with(Instant::now);
    }

    /// Whether the bound has been reached.
    pub(crate) fn expired(&self, bound: Duration) -> bool {
        self.waited().is_some_and(|waited| waited >= bound)
    }

    /// The words added to a refusal after a wait (`""` when nothing was waited): how long, and why it ended.
    pub(crate) fn suffix(&self, bound: Duration, why: &str) -> String {
        match self.waited() {
            None => String::new(),
            Some(waited) => format!(" (waited {}: {why})", seconds(waited.min(bound))),
        }
    }
}

/// `5 s`, `0.5 s`, `1.3 s`: whole seconds without a fraction.
pub(crate) fn seconds(duration: Duration) -> String {
    let tenths = (duration.as_millis() + 50) / 100;
    if tenths % 10 == 0 {
        format!("{} s", tenths / 10)
    } else {
        format!("{}.{} s", tenths / 10, tenths % 10)
    }
}

enum Decision {
    /// Look again (after a pause when one was waited for).
    Retry,
    /// Refuse, with this suffix after today's sentence.
    Refuse(String),
}

/// A create has found the name taken: wait, or refuse. **Refuse at once, with no suffix,** when waiting is off, when this process holds the
/// lock or the file's pid is this process's own, when the lock does not rank after every lock this thread holds, and when the holder does
/// not appear to be running (gone, reused, rebooted, another machine, an older prikk's body, a body still incomplete after a moment).
fn wait_decision(
    mutation_root: &MutationRoot,
    relative: &Path,
    path: &Path,
    kind: &str,
    rank: Rank,
    waiting: &mut Waiting,
) -> Decision {
    let bound = lock_wait_bound();
    if bound.is_zero() || this_process_holds(path) || this_thread_holds_at_or_after(rank) {
        return Decision::Refuse(String::new());
    }
    let bytes = match read_file_if_exists(mutation_root, relative) {
        // Released between the create and the read: try again at once.
        Ok(None) => return Decision::Retry,
        Ok(Some(bytes)) => bytes,
        Err(_) => return Decision::Refuse(waiting.suffix(bound, "the holder could not be read")),
    };
    let seen = crate::lock_holder::inspect(&bytes, &crate::lock_holder::SystemProbe);
    #[cfg(test)]
    test_hooks::note(format!("{:?}, {} bytes", seen.reason, bytes.len()));
    if seen.reason != crate::lock_holder::LivenessReason::IncompleteBody {
        // A body that was being written is written: the next holder's moment of an incomplete body is its own.
        waiting.incomplete_since = None;
    }
    match seen.reason {
        crate::lock_holder::LivenessReason::SameProcess => {
            let Some(pid) = seen.pid else {
                return Decision::Refuse(String::new());
            };
            if pid == std::process::id() {
                return Decision::Refuse(String::new());
            }
            waiting.begin();
            waiting.pid = Some(pid);
            if waiting.expired(bound) {
                return Decision::Refuse(
                    waiting.suffix(bound, &format!("pid {pid} still appears to be running")),
                );
            }
            waiting.announce_if_due(bound, || {
                format!("the {kind} lock held by pid {pid} (it appears to be running)")
            });
            std::thread::sleep(POLL);
            Decision::Retry
        }
        crate::lock_holder::LivenessReason::IncompleteBody => {
            // A live creator writes the body right after the create (measured: see `INCOMPLETE_GRACE`); a body that stays incomplete is not waited for.
            #[cfg(test)]
            test_hooks::look_at_an_incomplete_body();
            let since = *waiting.incomplete_since.get_or_insert_with(Instant::now);
            if since.elapsed() < incomplete_grace() {
                std::thread::sleep(Duration::from_millis(1));
                Decision::Retry
            } else {
                Decision::Refuse(waiting.suffix(bound, "the lock body stays incomplete"))
            }
        }
        reason => {
            // Not running (or never was, or cannot be judged): today's sentence; if this stop had been waiting, with what the judge now says.
            Decision::Refuse(waiting.suffix(
                bound,
                &match waiting.pid {
                    Some(pid) => format!("pid {pid}: {}", reason.sentence()),
                    None => reason.sentence().to_string(),
                },
            ))
        }
    }
}

/// How often and how long a denied create of a lock file is retried (Windows only; see
/// [`retry_denied_create`]).
const DENIED_CREATE_RETRIES: u32 = 50;
const DENIED_CREATE_PAUSE: Duration = Duration::from_millis(2);

/// Create the lock file exclusively. On Windows a create that is denied is retried briefly first;
/// everywhere else it is a single attempt, as before.
fn create_lock_file(
    mutation_root: &MutationRoot,
    relative: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let create = || create_lock_file_required(mutation_root, relative, bytes);
    if cfg!(windows) {
        retry_denied_create(
            create,
            || matches!(inspect_entry(mutation_root, relative), Ok(Some(_))),
            DENIED_CREATE_RETRIES,
            DENIED_CREATE_PAUSE,
        )
    } else {
        create()
    }
}

/// Windows answers `ERROR_ACCESS_DENIED` (os error 5), not "already exists", to a `CreateFile` that
/// meets a lock file another process has just deleted but whose removal has not finished (a file
/// marked for deletion cannot be opened until its last handle closes; `DeleteFile`'s remarks say
/// so). Two writers racing for one lock hit exactly that window: one releases (deletes) as the other
/// creates. The answer is transient, so a denied create is retried for a short, bounded time. If
/// the create then succeeds, the writer simply got the lock; if it reports the name exists, that is
/// the ordinary conflict. If it stays denied, the name decides: a lock name that is still there
/// (another process holds it, or its removal is stuck) is reported as the conflict it is
/// (`AlreadyExists`), and a name that is absent, or that cannot be inspected, is a real permission
/// problem and is reported as itself, denied, never as a conflict.
///
/// Compiled everywhere so its decisions are tested on every platform; only the Windows build of
/// [`create_lock_file`] calls it.
#[cfg_attr(not(windows), allow(dead_code))]
fn retry_denied_create(
    mut create: impl FnMut() -> std::io::Result<()>,
    mut lock_name_is_taken: impl FnMut() -> bool,
    retries: u32,
    pause: Duration,
) -> std::io::Result<()> {
    let mut attempts = 0;
    loop {
        match create() {
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                if attempts < retries {
                    attempts += 1;
                    std::thread::sleep(pause);
                    continue;
                }
                return if lock_name_is_taken() {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("the lock file is held or still being removed ({err})"),
                    ))
                } else {
                    Err(err)
                };
            }
            other => return other,
        }
    }
}

fn lock_body(kind: &str) -> String {
    crate::lock_holder::render_body(kind, std::process::id(), crate::lock_holder::own_facts())
}

// Repository mutation is implemented on Linux, macOS and Windows (`platform-support.md`); the
// tests that need a Linux-only or directory-sync mechanism carry their own narrower gate.
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wait_tests;
