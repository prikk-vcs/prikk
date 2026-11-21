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
use prikk_object::RefUpdatePayload;

use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::RepositoryLayout;
use crate::lock_holder::{self, PidLiveness, SystemProbe};

/// What the pointer lead of one ref is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lead {
    /// The lead is gone on the second read: the publication finished between the two.
    Gone,
    /// Another command holds the ref's lock and appears to be running: a publication in flight.
    InFlight {
        /// The holder's pid, from its lock body.
        pid: u32,
    },
    /// Interrupted (today's answer): no lock, or a lock whose holder does not appear to be running or cannot be judged.
    Interrupted {
        /// What the judge said about a lock that is still on disk (`None` when there is none).
        holder_note: Option<String>,
    },
}

/// Decide what `ref_name`'s lead is. The caller has read a lead; `still_leads` reads it again (after the holder is read here).
///
/// # Errors
///
/// A lock file or a ref container that cannot be read.
pub(crate) fn decide(
    layout: &RepositoryLayout,
    ref_name: &str,
    still_leads: impl FnOnce() -> Result<bool>,
) -> Result<Lead> {
    let holder = holder_of(layout, ref_name)?;
    if !still_leads()? {
        return Ok(Lead::Gone);
    }
    Ok(match holder {
        Holder::Running { pid } => Lead::InFlight { pid },
        Holder::NotRunning(note) => Lead::Interrupted {
            holder_note: Some(note),
        },
        Holder::NoLock => Lead::Interrupted { holder_note: None },
    })
}

/// Whether the ref's pointer still leads its log (the caller's second read). A missing pointer, or a pointer equal to the log's tip, does not.
pub(crate) fn lead_persists(layout: &RepositoryLayout, ref_name: &str) -> Result<bool> {
    let store = super::RefStore::new(layout.clone());
    let Some(pointer) = store.read_current_ref_state_id(ref_name)? else {
        return Ok(false);
    };
    let replay = store.replay_log(ref_name)?;
    let tip = match replay.records.last() {
        Some(record) => Some(
            RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?
                .new_ref_state_id,
        ),
        None => None,
    };
    Ok(tip != Some(pointer))
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
    let relative = layout.repository_relative(&layout.ref_lock_path(ref_name))?;
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
pub(crate) fn in_flight_sentence(ref_name: &str, pid: u32) -> String {
    format!("another command is publishing {ref_name} (pid {pid}, from its lock)")
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
    let name = pointer_replay
        .entries
        .iter()
        .rev()
        .find(|entry| entry.ref_name_key == key)
        .map(|entry| entry.ref_name.as_str());
    let name = match name {
        Some(name) => name,
        None => {
            let Some(log_name) = log_names.get(&key) else {
                return Ok(Some(super::incomplete_publication_refusal()));
            };
            let store = super::RefStore::new(layout.clone());
            if store.read_current_ref_state_id(log_name)?.is_none() {
                return Ok(Some(super::incomplete_publication_refusal()));
            }
            log_name.as_str()
        }
    };
    Ok(
        match decide(layout, name, || lead_persists(layout, name))? {
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
