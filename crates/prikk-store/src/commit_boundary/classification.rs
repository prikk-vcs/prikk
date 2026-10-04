//! RFC 166 D3: one classification, used by `verify`, `status`, `doctor`, both WAL repairs
//! (`--repair-wal-tail`, `--repair-tails`), and the checks `commit`, `rollback-draft` and `seal`
//! make before their first write.
//!
//! **Connectivity first** (D3 item 3): when the witness disagrees with the WAL (it names a seq the
//! WAL's own sound prefix does not reach, or the same seq with a different identity), the witnessed
//! Patch's own reachability from the witness's own ref is checked *before* concluding damage or
//! loss. The walk starts at the ref's current tip and stops at the witness's own recorded
//! `ref_tip_at_write` (§13 item 3's own correction over the design round's unbounded walk) -- so it
//! costs exactly the seals since the witness was written, normally one.
//!
//! **Rows 1-9 of RFC 166 §5 are this module's own job.** Row 10 (W3 disagreeing) is D4's: `verify`
//! calls [`verify_running_hash`](super::witness) *in addition* to this classification, since it is
//! the one O(WAL) check only a full `verify` pass is expected to pay for.

use prikk_error::Result;
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload};

use super::active::ActiveRefMetadata;
use super::witness::{WitnessRecord, WitnessState};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::refs::RefStore;
use crate::wal::WalReplay;

/// RFC 166 §5's own ten rows, 1-9 here (row 10, W3, is layered on by `verify` itself -- see the
/// module doc). Named for what each one *means*, not for its own row number, since the number is
/// RFC prose, not a user-facing or even an internal identifier anything branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// §1: no witness at all (never had one, or one removed), **or** a witness connectivity already
    /// confirmed is a drain it missed (`stale: true`) -- 0.48.0's own rule 3 for this session, exactly,
    /// either way. Never worse than 0.48.0 (C2).
    NoWitness {
        /// Whether the WAL itself (ignoring the witness entirely) reads sound today -- the exact
        /// question 0.48.0's own reader already asks, carried through unchanged.
        wal_otherwise_sound: bool,
        /// §13 item 16: `true` when this is specifically a witness connectivity confirmed as a drain
        /// an older binary's own seal left behind (not a genuinely absent one). Silent in `status`
        /// either way (item 16); `doctor` alone notes the `true` case, since the next commit or
        /// `--repair-tails` replaces it.
        stale: bool,
    },
    /// Row 1: the WAL is sound and the witness agrees with its own last record.
    Healthy {
        /// The WAL's own last sound, acknowledged sequence number.
        last_seq: u64,
    },
    /// Row 2: the WAL is sound, ahead of the witness (a crash before acknowledgement, or an older
    /// binary's own commit the witness never saw). Not a problem: the next commit advances it.
    Pending {
        /// The witness's own last covered sequence number, if any.
        witnessed_seq: Option<u64>,
        /// The WAL's own last sound sequence number, ahead of `witnessed_seq`.
        last_seq: u64,
    },
    /// Row 3: a genuine crash-shaped tail past the witness's own last covered record, which agrees
    /// with the sound prefix. Never acknowledged; the existing repair removes it as it does today.
    CrashTail {
        /// The sound prefix's own last sequence number, which the witness agrees with.
        sound_through: Option<u64>,
    },
    /// Row 4 (N6): the record right after the witnessed prefix is damaged (folded into the WAL's
    /// own tail by rule 3) or missing, and the witness names it as acknowledged, with connectivity
    /// failing to explain it as a drain. The repair must refuse; round 2's `--discard-damaged-
    /// commits` is the way out.
    AcknowledgedDamage {
        /// The acknowledged sequence number the witness names, which is no longer sound in the WAL.
        witnessed_seq: u64,
    },
    /// Row 5: the WAL is shorter than, or empty relative to, the witness, and connectivity fails --
    /// an acknowledged record with nothing durable left of it. Same refusal and way out as row 4.
    AcknowledgedLoss {
        /// The acknowledged sequence number the witness names, no longer present in the WAL at all.
        witnessed_seq: u64,
    },
    /// Row 6: the witness names the WAL's own last sound seq, but a different Patch id or frame
    /// hash, and connectivity fails to explain it. Not a crash shape -- no repair verb; a copy is
    /// the way out.
    SubstitutedRecord {
        /// The sequence number both the WAL and the witness agree on, whose identity they disagree
        /// on.
        witnessed_seq: u64,
    },
    /// Row 7: the WAL has a tail *and* the witness is damaged, so a reader cannot tell whether the
    /// tail is a genuine crash or acknowledged damage the witness would otherwise have named.
    /// Fails closed as "unknown" -- the repair refuses, naming the same way out as rows 4/5.
    UnknownWithDamagedWitness,
    /// Row 8: the WAL is wholly sound with no tail, and the witness is damaged. Nothing is at risk
    /// (the witness only ever decides tails), so this is a warning, not a refusal; `--repair-tails`
    /// rebuilds the witness from the sound WAL (§13 item 5).
    WitnessDamaged,
    /// Row 9 (§1.6's own shape): the WAL has records but no durable, valid owner names whose they
    /// are. Takes priority over every witness-based verdict, since nothing else can be resolved
    /// without an owner. Refuses as today; round 2's `--restore-queue-target` is the way out.
    OwnershipMissing,
}

/// D3's own classification. `owning_ref` is the session's own ownership read (`ActiveRefMetadata`),
/// already performed by the caller (every caller already needs it for its own purposes, and D6's
/// own ref-name-vs-witness check needs both values side by side, not re-derived here).
pub fn classify(
    layout: &RepositoryLayout,
    replay: &WalReplay,
    owning_ref: &ActiveRefMetadata,
    witness: &WitnessState,
) -> Result<Verdict> {
    let last_sound_seq = replay.records.last().map(|record| record.seq);
    let there_is_unexplained_tail = replay.trailing_partial_bytes > 0;

    // Row 9 first: with no durable owner, nothing else here can even be asked meaningfully (D3's
    // own precedence -- RFC 166 §5 row 9 applies "any" witness state).
    if !replay.records.is_empty() && !matches!(owning_ref, ActiveRefMetadata::Valid(_)) {
        return Ok(Verdict::OwnershipMissing);
    }
    // D6: `ref-name` is checked against the witness's own ref name. A session cannot have two
    // owners; a mismatch is damage, classified the same as no owner at all (row 9's own refusal).
    if let (ActiveRefMetadata::Valid(owning_name), WitnessState::Valid(witness_record)) =
        (owning_ref, witness)
    {
        if owning_name != &witness_record.ref_name {
            return Ok(Verdict::OwnershipMissing);
        }
    }

    let witness_record = match witness {
        WitnessState::Damaged(_) => {
            return Ok(if there_is_unexplained_tail {
                Verdict::UnknownWithDamagedWitness // row 7
            } else {
                Verdict::WitnessDamaged // row 8
            });
        }
        WitnessState::Absent => {
            return Ok(Verdict::NoWitness {
                wal_otherwise_sound: !replay.has_item_failure(),
                stale: false,
            });
        }
        WitnessState::Valid(record) => record,
    };

    // D6: the witness's own ref name is checked against the session's durable owner. A mismatch
    // here is damage (handled by the caller wiring D6 in; this function's own job past this point
    // assumes the two already agree, which every call site confirms before calling `classify`).

    if let Some(last_seq) = last_sound_seq {
        if witness_record.last_seq == last_seq {
            if identity_matches(replay, witness_record) {
                return Ok(if there_is_unexplained_tail {
                    Verdict::CrashTail {
                        sound_through: Some(last_seq),
                    } // row 3
                } else {
                    Verdict::Healthy { last_seq } // row 1
                });
            }
            // Same seq, different identity -- row 6, unless connectivity explains it as a drain.
            if patch_is_sealed_and_reachable_since(
                layout,
                &witness_record.ref_name,
                witness_record.patch_id,
                witness_record.ref_tip_at_write,
            )? {
                return Ok(Verdict::NoWitness {
                    wal_otherwise_sound: !replay.has_item_failure(),
                    stale: true,
                });
            }
            return Ok(Verdict::SubstitutedRecord {
                witnessed_seq: witness_record.last_seq,
            });
        }
        if witness_record.last_seq < last_seq {
            return Ok(Verdict::Pending {
                witnessed_seq: Some(witness_record.last_seq),
                last_seq,
            }); // row 2
        }
        // witness_record.last_seq > last_seq: ahead of the sound prefix.
        if there_is_unexplained_tail && witness_record.last_seq == last_seq.saturating_add(1) {
            // The witness names exactly the record rule 3 folded into the tail -- acknowledged
            // damage, unless connectivity shows it was actually sealed (a drain this reader missed).
            if patch_is_sealed_and_reachable_since(
                layout,
                &witness_record.ref_name,
                witness_record.patch_id,
                witness_record.ref_tip_at_write,
            )? {
                return Ok(Verdict::NoWitness {
                    wal_otherwise_sound: !replay.has_item_failure(),
                    stale: true,
                });
            }
            return Ok(Verdict::AcknowledgedDamage {
                witnessed_seq: witness_record.last_seq,
            }); // row 4
        }
    } else if there_is_unexplained_tail && witness_record.last_seq == 1 {
        // No sound record at all -- the session's own first record is itself the damaged one.
        if patch_is_sealed_and_reachable_since(
            layout,
            &witness_record.ref_name,
            witness_record.patch_id,
            witness_record.ref_tip_at_write,
        )? {
            return Ok(Verdict::NoWitness {
                wal_otherwise_sound: !replay.has_item_failure(),
                stale: true,
            });
        }
        return Ok(Verdict::AcknowledgedDamage {
            witnessed_seq: witness_record.last_seq,
        });
    }

    // The WAL is shorter than, or empty relative to, the witness, with no unexplained tail to
    // account for the gap (a restored backup, or a drain -- row 5 unless connectivity says drain).
    if patch_is_sealed_and_reachable_since(
        layout,
        &witness_record.ref_name,
        witness_record.patch_id,
        witness_record.ref_tip_at_write,
    )? {
        return Ok(Verdict::NoWitness {
            wal_otherwise_sound: !replay.has_item_failure(),
            stale: true,
        });
    }
    Ok(Verdict::AcknowledgedLoss {
        witnessed_seq: witness_record.last_seq,
    }) // row 5
}

/// Whether the WAL's own sound record at `witness.last_seq` is the exact one the witness names --
/// both its Patch id and its own frame hash, not only the seq (Q3's own same-seq-substitution
/// shape).
fn identity_matches(replay: &WalReplay, witness: &WitnessRecord) -> bool {
    let Some(record) = replay
        .records
        .iter()
        .find(|record| record.seq == witness.last_seq)
    else {
        return false;
    };
    if record.envelope.object_id() != witness.patch_id {
        return false;
    }
    crate::wal::record_frame_checksum(record).is_ok_and(|hash| hash == witness.frame_hash)
}

/// D3 item 3 / §13 item 3: is `patch_id` sealed into a block reachable from `ref_name`'s own
/// current tip, walking back only as far as `stop_at` (the witness's own recorded tip at the time
/// it was written) -- the seals since the witness was written, not an unbounded walk to genesis.
fn patch_is_sealed_and_reachable_since(
    layout: &RepositoryLayout,
    ref_name: &str,
    patch_id: ObjectId,
    stop_at: Option<ObjectId>,
) -> Result<bool> {
    let ref_store = RefStore::new(layout.clone());
    let objects = FileObjectStore::new(layout.clone());
    let mut current = ref_store.read_current_ref_state_id(ref_name)?;
    loop {
        let Some(ref_state_id) = current else {
            return Ok(false);
        };
        if Some(ref_state_id) == stop_at {
            return Ok(false);
        }
        let Some(envelope) = objects.read_typed(ref_state_id, ObjectType::RefState)? else {
            return Ok(false);
        };
        let Ok(ref_state) =
            RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        else {
            return Ok(false);
        };
        if let Some(block_envelope) =
            objects.read_typed(ref_state.target_object_id, ObjectType::Block)?
        {
            if let Ok(block) = BlockPayload::decode_canonical(&block_envelope.canonical_payload) {
                if block.patch_ids.contains(&patch_id) {
                    return Ok(true);
                }
            }
        }
        current = ref_state.previous_ref_state_id;
    }
}

/// RFC 166: the exact refusal text `commit`, `rollback-draft` and `seal`'s own pre-write checks use
/// when [`classify`] returns a blocking verdict (RFC 166 §5 rows 4, 5, 6, 7, 9) -- `None` for every
/// verdict that does not block a write (rows `NoWitness`, 1, 2, 3, 8). Centralized so the call sites
/// cannot drift to different sentences for the same row. **Rows 4, 5 and 7 name
/// `prikk doctor --discard-damaged-commits`** (round 2, D5, §13 item 14) -- the one way out for
/// acknowledged damage or loss. **Row 9 names `prikk doctor --restore-queue-target --ref <ref>`**
/// (round 2, D5, §13 item 15), covering both an outright missing owner and D6's own
/// ref-name-vs-witness mismatch, which this function folds into the same verdict. **Row 6 names no
/// verb**: a substituted record (and D4's own row 10) is not a crash shape, so a copy is the way
/// out.
#[must_use]
pub fn write_refusal_reason(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::NoWitness { .. }
        | Verdict::Healthy { .. }
        | Verdict::Pending { .. }
        | Verdict::CrashTail { .. }
        | Verdict::WitnessDamaged => None,
        Verdict::AcknowledgedDamage { witnessed_seq } => Some(format!(
            "a queued commit you were told had succeeded (sequence {witnessed_seq}) is damaged; \
             it was already acknowledged, so it cannot be removed as a crash leftover -- run \
             `prikk doctor --discard-damaged-commits` to remove it instead"
        )),
        Verdict::AcknowledgedLoss { witnessed_seq } => Some(format!(
            "a queued commit you were told had succeeded (sequence {witnessed_seq}) is no \
             longer present at all; it was already acknowledged, so it cannot be removed as a \
             crash leftover -- run `prikk doctor --discard-damaged-commits` to declare it lost \
             instead"
        )),
        Verdict::SubstitutedRecord { witnessed_seq } => Some(format!(
            "sequence {witnessed_seq} does not match the queued commit you were told had \
             succeeded; this is not a crash shape, and a copy is the way out"
        )),
        Verdict::UnknownWithDamagedWitness => Some(
            "the queue has an unexplained tail, and this session's own acknowledgment history is \
             unreadable, so the tail cannot be shown to be a crash leftover -- run `prikk doctor \
             --discard-damaged-commits` to discard it"
                .to_string(),
        ),
        Verdict::OwnershipMissing => Some(
            "queued commits exist but no durable, matching owner names them -- run \
             `prikk doctor --restore-queue-target --ref <ref>` to give the queue its owner back"
                .to_string(),
        ),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
