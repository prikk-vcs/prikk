//! The ref log, summarised per ref (0.59.0 P5a2).
//!
//! A publication used to walk the whole ref log three times per claim: the precondition decoded every record's `RefUpdatePayload` (twice, and a
//! third time inside the publication), and `classify_state` copied one ref's whole subsequence and decoded it again to check the chain. A `sync seal` of
//! 10,000 claims spent 53% of its time there (P5a, measured), and a hold kept every decoded envelope (about 1.4 KB per sealed block).
//!
//! **What the precondition and the classification ask of the log, and so what is kept** -- for **every ref**, one [`RefChain`]:
//! - the newest record's `new_ref_state_id` and `ref_name` (the precondition's `newest_log` and `log_names`);
//! - how many records the ref has, the id before the newest, and the **newest record's envelope** (`classify_state`'s `log_position`: the log's tip,
//!   the tip before it, and whether the last record is exactly the publication's `RefUpdate`);
//! - whether the chain is **sound to the tip**: every record named the same ref, chained to the one before (`old_ref_state_id` = the previous
//!   `new_ref_state_id`, `None` for the first), carried `update_seq == position + 1` and `created_at == 0`. These are `log_position`'s own four checks;
//! - and, for the whole log, whether any record carried `created_at != 0` (the precondition's own refusal).
//!
//! **No other record is kept.** The summary is built from a whole decode of a log that is **clean** (every frame evaluated, no trailing partial
//! record) and is extended by the appended tail only, under `kept_ref_log`'s staleness protocol (stat, canary, tail).
//!
//! **It fails closed.** A summary never produces a refusal: it answers "this is the state a sound log has" or it is not used. Any doubt -- a payload that
//! does not decode, a chain that is not sound, a ref whose records name two names, a publication for a name the chain does not carry -- falls back to the
//! whole walk, whose refusal text is the one every caller has always printed. Each fallback is counted ([`Fallback`]) so a test can say which.

use std::collections::BTreeMap;

use prikk_error::Result;
use prikk_object::{ObjectEnvelope, ObjectId, RefUpdatePayload};

use super::container::RefContainerRecord;

/// Why a use of the summary did not stand and the whole walk ran instead. Each is a doubt, said where it is raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// Nothing was kept (the first use under a hold, or outside a hold).
    NothingKept,
    /// The log is another file than the kept copy's (another inode), or shorter than it (truncated or replaced).
    FileChanged,
    /// The log grew, but the copy's last record is not where it was (the canary): a rewrite below the copy's extent.
    CanaryDiffers,
    /// The appended tail has a damaged frame or a trailing partial record: it is never added to the copy.
    TailNotClean,
    /// A record's payload does not decode, so it cannot be summarised.
    PayloadUndecodable,
    /// The ref's chain is not sound to the tip (a record out of sequence, a different old id, a created_at, a duplicate).
    ChainUnsound,
    /// The ref's records name more than one ref name, or not the publication's.
    NameMismatch,
}

#[cfg(test)]
std::thread_local! {
    static FALLBACKS: std::cell::RefCell<Vec<Fallback>> = const { std::cell::RefCell::new(Vec::new()) };
    /// A control (`cfg(test)`): the summary is never used, every use takes the whole walk.
    static FORCE_WHOLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Count one fallback (`cfg(test)`: nothing in a shipped build).
#[cfg(test)]
pub(super) fn count_fallback(reason: Fallback) {
    FALLBACKS.with(|list| list.borrow_mut().push(reason));
}

/// Count one fallback (nothing in a shipped build).
#[cfg(not(test))]
pub(super) fn count_fallback(_reason: Fallback) {}

/// The fallbacks counted on this thread since the last call (`cfg(test)`), emptied.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn take_fallbacks_for_test() -> Vec<Fallback> {
    FALLBACKS.with(|list| std::mem::take(&mut *list.borrow_mut()))
}

/// Run `body` with the summary never used (`cfg(test)`): the oracle of the differential tests and a control.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn with_whole_walk_for_test<T>(body: impl FnOnce() -> T) -> T {
    FORCE_WHOLE.with(|flag| flag.set(true));
    let result = body();
    FORCE_WHOLE.with(|flag| flag.set(false));
    result
}

/// Whether the summary may be used at all (`cfg(test)` can switch it off).
pub(super) fn enabled() -> bool {
    #[cfg(test)]
    let on = !FORCE_WHOLE.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let on = true;
    on
}

/// What the log says of one ref.
#[derive(Debug, Clone)]
pub(in crate::refs) struct RefChain {
    /// The `ref_name` of the ref's first record: every record of a sound chain names it.
    pub(in crate::refs) name: String,
    /// The `ref_name` of the newest record (the precondition's `log_names`).
    pub(in crate::refs) last_name: String,
    /// How many records the ref has.
    pub(in crate::refs) count: u64,
    /// The newest record's `new_ref_state_id`.
    pub(in crate::refs) tip: ObjectId,
    /// The `new_ref_state_id` of the record before it, `None` when it is the first.
    pub(in crate::refs) before_last: Option<ObjectId>,
    /// Every record so far named `name`, chained to the one before, carried `update_seq == position + 1` and `created_at == 0`.
    pub(in crate::refs) sound: bool,
    /// The newest record's envelope, as the log holds it.
    pub(in crate::refs) last: ObjectEnvelope,
}

/// The ref log, summarised.
#[derive(Debug, Clone, Default)]
pub(in crate::refs) struct RefLogSummary {
    chains: BTreeMap<[u8; 32], RefChain>,
    /// Some record (of any ref) carries `created_at != 0`.
    created_at_bad: bool,
}

impl RefLogSummary {
    /// Add `records` (the log's next records, in file order) to the summary.
    ///
    /// # Errors
    ///
    /// A record whose payload does not decode: the summary cannot answer for it, and the caller does not use one.
    pub(in crate::refs) fn extend(&mut self, records: &[RefContainerRecord]) -> Result<()> {
        for record in records {
            let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
            if update.created_at != 0 {
                self.created_at_bad = true;
            }
            match self.chains.get_mut(&record.ref_name_key) {
                Some(chain) => {
                    let in_sequence = update.ref_name == chain.name
                        && update.old_ref_state_id == Some(chain.tip)
                        && chain.count.checked_add(1) == Some(update.update_seq)
                        && update.created_at == 0;
                    chain.sound &= in_sequence;
                    chain.count += 1;
                    chain.before_last = Some(chain.tip);
                    chain.tip = update.new_ref_state_id;
                    chain.last_name = update.ref_name;
                    chain.last = record.envelope.clone();
                }
                None => {
                    let first = update.old_ref_state_id.is_none()
                        && update.update_seq == 1
                        && update.created_at == 0;
                    self.chains.insert(
                        record.ref_name_key,
                        RefChain {
                            name: update.ref_name.clone(),
                            last_name: update.ref_name,
                            count: 1,
                            tip: update.new_ref_state_id,
                            before_last: None,
                            sound: first,
                            last: record.envelope.clone(),
                        },
                    );
                }
            }
        }
        Ok(())
    }

    /// Some record carries `created_at != 0`: the precondition refuses.
    pub(in crate::refs) fn created_at_bad(&self) -> bool {
        self.created_at_bad
    }

    /// Every ref's chain, by name key.
    pub(in crate::refs) fn chains(&self) -> impl Iterator<Item = (&[u8; 32], &RefChain)> {
        self.chains.iter()
    }

    /// One ref's chain.
    pub(in crate::refs) fn chain(&self, key: &[u8; 32]) -> Option<&RefChain> {
        self.chains.get(key)
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;
