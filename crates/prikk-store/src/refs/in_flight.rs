//! Is a ref's pointer lead a publication **in flight**, or one that was **interrupted**? (0.58.0 F2.)
//!
//! A publication writes the ref's pointer, then its log record, under the ref's `RefLock` (and the pointer-index and log container locks),
//! released after the log append. For the time between the two writes the pointer leads the log -- the same bytes a crash leaves, and a
//! reader that takes no lock cannot tell them apart. Before this module the write-path precondition, `ref complete`, `verify` and `doctor` all read a live
//! publication as an interrupted one (measured on 0.57.0 and the S5b build by `tag create` racing a `sync seal`: 275 and 171 refusals in 400 rounds).
//!
//! **One function decides: [`decide`].** The ref's `RefLock` is the holder's mark (the publisher takes it first and releases it last); its
//! body names the process, and `lock_holder`'s judge says whether that process appears to be running. **Only "appears running" is in flight.**
//! "Does not appear to be running", "unknown" (another pid namespace, an older prikk's body, a body still being written) and no lock at all keep
//! today's answer -- an interrupted publication -- and the answer adds what the judge said about the lock that is still there. That is 0.58.0
//! step 0a's rule: only a definite answer changes behaviour.
//!
//! **The order is: the lead, then the holder, then the lead again.** The caller has read the lead; [`decide`] reads the holder, then asks the
//! caller whether the lead is still there. A publication that finished in between has no lead on the second read, and the writer proceeds -- it is
//! neither in flight nor interrupted. It reads no more than the lock file (one `stat` and one small read) and writes nothing; it takes no lock.
//!
//! **Why the `RefLock` and not the container locks.** The container locks are held by any publication of any ref; a lead of `tags/t1`
//! must not be called in flight because a publication of `heads/x` holds them. The `RefLock` is per ref.

use prikk_error::Result;
use prikk_object::{ObjectId, RefUpdatePayload};

use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::RepositoryLayout;
use crate::lock_holder::{self, PidLiveness, SystemProbe};

/// What the pointer lead of one ref is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lead {
    /// The lead is gone on the second read: the publication finished between the two.
    Gone,
    /// Another command is publishing the ref: its lock is held by a process that appears to be running, **or the lead changed between the two reads** (a
    /// pointer that moves is proof something is writing it; a crash cannot do that).
    InFlight {
        /// The holder's pid, from its lock body, when a holder read found a running one; `None` when the lead changed and no holder read named one.
        pid: Option<u32>,
    },
    /// Interrupted (today's answer): no lock, or a lock whose holder does not appear to be running or cannot be judged.
    Interrupted {
        /// What the judge said about a lock that is still on disk (`None` when there is none).
        holder_note: Option<String>,
    },
}

/// Decide what `ref_name`'s lead is. **A lead is identified by its RefState id.** The caller has read lead `first`; `lead_now` reads it again
/// (after the holder is read here) and answers with the pointer's RefState id while the pointer leads the log, `None` when it does not.
///
/// - **No lead now:** [`Lead::Gone`] -- the publication finished.
/// - **A different lead now:** in flight. A pointer that changed between two reads is a publisher running; the holder read made before it cannot
///   speak for a lead that did not yet exist (a `sync seal` publishes one claim per publication, back to back, each under its own lock, so a holder
///   read can land in the gap between two). The pid is the first holder read's, if it named a running one.
/// - **The same lead, a holder that appeared running:** in flight.
/// - **The same lead, a holder that did not:** the holder is read **once more, after** that second lead read. A lead that existed at two moments with
///   no running holder at either cannot be a running publication: interrupted, with what the later read said. **No loop.**
///
/// # Errors
///
/// A lock file or a ref container that cannot be read.
pub(crate) fn decide(
    layout: &RepositoryLayout,
    ref_name: &str,
    first: ObjectId,
    lead_now: impl FnOnce() -> Result<Option<ObjectId>>,
) -> Result<Lead> {
    let holder = holder_of(layout, ref_name)?;
    let Some(second) = lead_now()? else {
        return Ok(Lead::Gone);
    };
    if second != first {
        return Ok(Lead::InFlight {
            pid: match holder {
                Holder::Running { pid } => Some(pid),
                _ => None,
            },
        });
    }
    if let Holder::Running { pid } = holder {
        return Ok(Lead::InFlight { pid: Some(pid) });
    }
    Ok(match holder_of(layout, ref_name)? {
        Holder::Running { pid } => Lead::InFlight { pid: Some(pid) },
        Holder::NotRunning(note) => Lead::Interrupted {
            holder_note: Some(note),
        },
        Holder::NoLock => Lead::Interrupted { holder_note: None },
    })
}

/// The ref's lead as of now: the RefState id the pointer names while it differs from the log's tip, `None` when the pointer is missing or equals the
/// log's tip (no lead).
pub(crate) fn lead_now(layout: &RepositoryLayout, ref_name: &str) -> Result<Option<ObjectId>> {
    let store = super::RefStore::new(layout.clone());
    // **The log first, then the pointer** (the reverse of a publication's write order, 0.59.0 RS2a): the pointer is then at least as new as the tip it
    // is compared with, so a difference is the pointer ahead -- a lead. Read the other way round, a publication that completes between the two reads
    // (a `sync seal` publishes claim after claim) leaves a pointer **behind** the log: a lead that was never there, which `decide` then called interrupted.
    let replay = store.replay_log(ref_name)?;
    let tip = match replay.records.last() {
        Some(record) => Some(
            RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?
                .new_ref_state_id,
        ),
        None => None,
    };
    #[cfg(test)]
    super::view_hooks::run("lead-between-log-and-pointer");
    let Some(pointer) = store.read_current_ref_state_id(ref_name)? else {
        return Ok(None);
    };
    Ok((tip != Some(pointer)).then_some(pointer))
}

/// The note an interrupted answer adds about a lock that is still on disk (`None` when there is no lock, or the holder appears running).
///
/// # Errors
///
/// A lock file that cannot be read.
pub(crate) fn holder_note(layout: &RepositoryLayout, ref_name: &str) -> Result<Option<String>> {
    Ok(match holder_of(layout, ref_name)? {
        Holder::NotRunning(note) => Some(note),
        Holder::Running { .. } | Holder::NoLock => None,
    })
}

enum Holder {
    NoLock,
    Running { pid: u32 },
    NotRunning(String),
}

fn holder_of(layout: &RepositoryLayout, ref_name: &str) -> Result<Holder> {
    holder_at(layout, &layout.ref_lock_path(ref_name))
}

/// The pid of the process that holds the **active** lock of the default session and appears to be running, if any (0.59.0 RS2c). A `seal`, a `commit` and
/// a `sync seal` take it for the whole of their work, so a reader that finds the queue between a publish and its drain, or in the middle of a commit's
/// three writes, asks it: only "appears running" is in flight; no lock, a holder that is gone, and one that cannot be judged are today's answer.
pub(crate) fn active_publisher(layout: &RepositoryLayout) -> Result<Option<u32>> {
    Ok(
        match holder_at(
            layout,
            &layout.active_lock_path(crate::foundation::layout::DEFAULT_ACTIVE_NAME),
        )? {
            // Never this process: a command that holds the active lock itself (`commit`, `seal`, `branch create`) asks the same question about
            // the queue it is about to write, and the queue's state is then not "another command publishing" but what it finds.
            Holder::Running { pid } if pid != std::process::id() => Some(pid),
            Holder::Running { .. } | Holder::NoLock | Holder::NotRunning(_) => None,
        },
    )
}

/// Whether the lock of `ref_name` is held by **another** process that appears to be running: a command is publishing the ref right now (0.59.0 RS2c,
/// for a conclusion of the form "an object is held and no ref targets it": the object is written before its pointer, under the ref's lock).
pub(crate) fn ref_publisher_running(layout: &RepositoryLayout, ref_name: &str) -> Result<bool> {
    Ok(matches!(
        holder_of(layout, ref_name)?,
        Holder::Running { pid } if pid != std::process::id()
    ))
}

fn holder_at(layout: &RepositoryLayout, lock_path: &std::path::Path) -> Result<Holder> {
    let relative = layout.repository_relative(lock_path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(Holder::NoLock);
    };
    let seen = lock_holder::inspect(&bytes, &SystemProbe);
    let liveness = seen.reason.liveness();
    Ok(match (liveness, seen.pid) {
        (PidLiveness::AppearsRunning, Some(pid)) => Holder::Running { pid },
        _ => Holder::NotRunning(lock_holder::describe_liveness(liveness, seen.reason)),
    })
}

/// The words of the in-flight refusal, shared by the precondition, `ref complete` and the readers' finding.
#[must_use]
pub(crate) fn in_flight_sentence(ref_name: &str, pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("{IN_FLIGHT_PREFIX}{ref_name} (pid {pid}, from its lock)"),
        None => format!("{IN_FLIGHT_PREFIX}{ref_name}"),
    }
}

/// The first words of every in-flight refusal: [`in_flight_sentence`] begins with them, and nothing else does.
const IN_FLIGHT_PREFIX: &str = "another command is publishing ";

/// The text of `error` when it is an in-flight refusal of the write-path precondition (a `Precondition` that begins as [`in_flight_sentence`] does).
#[must_use]
pub(crate) fn in_flight_text(error: &prikk_error::PrikkError) -> Option<&str> {
    match error {
        prikk_error::PrikkError::Precondition(text) if text.starts_with(IN_FLIGHT_PREFIX) => {
            Some(text)
        }
        _ => None,
    }
}

/// What a waiting writer says it is waiting for, from an in-flight refusal's text: `another command to finish publishing tags/t1 (pid 1234, from its lock)`.
#[must_use]
pub(crate) fn waiting_for(text: &str) -> String {
    let sentence = text.split(';').next().unwrap_or(text);
    match sentence.strip_prefix(IN_FLIGHT_PREFIX) {
        Some(rest) => format!("another command to finish publishing {rest}"),
        None => sentence.to_string(),
    }
}

/// Run the write-path precondition until it stops refusing for a publication in flight, for at most the wait's bound.
/// Everything else it returns is returned at once, so is an in-flight refusal when the wait is off, when this thread holds a lock a publisher could be
/// waiting for (`lock::Rank::Ref` and after: the one rule of `lock.rs`), and when the publisher is this very process. After the bound the refusal
/// is returned with the words for the wait; after one second one line goes to stderr.
pub(crate) fn wait_while_in_flight(
    layout: &RepositoryLayout,
    exclude_ref_name: Option<&str>,
    check_ref_log_tail: bool,
    mut replay_out: Option<&mut Option<crate::wal::WalReplay>>,
) -> Result<()> {
    let setting = crate::lock::lock_wait_setting();
    let bound = setting.bound();
    let mut waiting = crate::lock::Waiting::new();
    let mut pause = std::time::Duration::from_millis(5);
    loop {
        let attempt = super::ensure_publication_precondition_once(
            layout,
            exclude_ref_name,
            check_ref_log_tail,
            replay_out.as_deref_mut(),
        );
        let error = match attempt {
            Err(error) => error,
            done => return done,
        };
        let Some(text) = in_flight_text(&error) else {
            return Err(error);
        };
        if setting == crate::lock::LockWait::Off
            || names_this_process(text)
            || crate::lock::this_thread_holds_at_or_after(crate::lock::Rank::Ref)
        {
            return Err(error);
        }
        // A wait would begin here: a mis-set variable refuses at once and says why.
        if let crate::lock::LockWait::Misset(got) = &setting {
            return Err(prikk_error::PrikkError::Precondition(format!(
                "{text}{}",
                crate::lock::not_waited_suffix(got)
            )));
        }
        waiting.begin();
        if waiting.expired(bound) {
            let suffix = waiting.suffix(bound, "the other command is still publishing");
            return Err(prikk_error::PrikkError::Precondition(format!(
                "{text}{suffix}"
            )));
        }
        waiting.announce_if_due(bound, || waiting_for(text));
        std::thread::sleep(pause);
        pause = (pause * 2).min(std::time::Duration::from_millis(50));
    }
}

/// Whether an in-flight refusal names **this** process as the publisher (`(pid N, from its lock)`): a wait would be for itself.
#[must_use]
pub(crate) fn names_this_process(text: &str) -> bool {
    text.contains(&format!("(pid {}, from its lock)", std::process::id()))
}

/// The precondition's answer for one ref whose newest pointer and newest log record disagree (`key` names it): `None` when the lead is gone
/// on the second read (carry on), the in-flight refusal for a publisher that appears to be running, else today's interrupted-publication refusal.
///
/// **A log record with no pointer entry** (0.59.0 RS2a): the caller reads the log, then the pointer index, and a publication writes the pointer
/// first, so a record the log read saw already has its pointer entry in the pointer read that follows -- unless the pointer index holds none for
/// it. That is the log leading: damage, or a publication whose pointer write landed after the pointer read. It gets the second read a pointer lead
/// gets: the pointer is read again; if it is still absent the answer is today's refusal (a log with no pointer is not a lead), and if it has
/// appeared the ref is judged as any pointer lead is (`decide`: the holder, then the lead again -- gone, in flight, or interrupted).
/// `log_names` is the name each log record carries, for the key the pointer index has no entry for.
pub(super) fn refusal_for_mismatch(
    layout: &RepositoryLayout,
    pointer_replay: &super::pointer_index::PointerIndexReplay,
    log_names: &std::collections::BTreeMap<[u8; 32], String>,
    key: [u8; 32],
) -> Result<Option<prikk_error::PrikkError>> {
    let entry = pointer_replay
        .entries
        .iter()
        .rev()
        .find(|entry| entry.ref_name_key == key);
    // The name and the first lead: the pointer's RefState id **as the caller's read saw it**. A log record with no pointer entry gets the second
    // read here, and that read is its first lead.
    let (name, first) = match entry {
        Some(entry) => (entry.ref_name.as_str(), entry.ref_state_id),
        None => {
            let Some(log_name) = log_names.get(&key) else {
                return Ok(Some(super::incomplete_publication_refusal()));
            };
            let store = super::RefStore::new(layout.clone());
            let Some(pointer) = store.read_current_ref_state_id(log_name)? else {
                return Ok(Some(super::incomplete_publication_refusal()));
            };
            (log_name.as_str(), pointer)
        }
    };
    Ok(
        match decide(layout, name, first, || lead_now(layout, name))? {
            Lead::Gone => None,
            Lead::InFlight { pid } => Some(prikk_error::PrikkError::Precondition(format!(
                "{}; run this command again when it finishes",
                in_flight_sentence(name, pid)
            ))),
            Lead::Interrupted { .. } => Some(super::incomplete_publication_refusal()),
        },
    )
}

/// The code `verify` reports for a lead whose publisher is running. A **new id**, not the old one at a lower severity: `PRIKK-VERIFY-REF-POINTER-LEADS-LOG`
/// keeps meaning "interrupted, blocks, run `ref complete`" for anything that keys on it, and a reader that sees this one is told to do nothing.
pub(crate) const IN_FLIGHT_CODE: &str = "PRIKK-VERIFY-REF-PUBLICATION-IN-FLIGHT";

#[cfg(test)]
mod tests;
