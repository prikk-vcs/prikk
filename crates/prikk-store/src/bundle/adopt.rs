//! `bundle import --adopt` (0.57.0 S1b, the owner's ruling A): a delivered branch that is absent locally and **signed by a key
//! this repository has adopted, throughout its RefState chain**, lands as a real local branch, instead of under `remotes/`.
//!
//! **Why this is not `archive import --adopt`'s publish loop.** An archive carries the sender's own ref log (a `RefUpdate` per
//! step, signed by the sender), and `archive import --adopt` republishes it. A bundle carries RefStates and no RefUpdates, so
//! the receiver signs its own `RefUpdate` over each delivered, adopted-key-signed `RefState`, oldest first: the shape
//! `ref complete` already publishes (a `RefPublication` whose update names the completing key, over a RefState it never
//! re-signs) and the one `verify` accepts. The local ref log then records that **the receiver moved its branch**; the original
//! signers stay on the RefStates, and on the Blocks they point at.
//!
//! **What is shared with the archive, and not copied here:** `adopt_local_ref::begin_adoption` (the ref is absent; the active
//! queue does not own it; the received pointer's class decides) and `adopt_local_ref::capture_for_current_branch` (the clean
//! worktree, nothing in the way, the switch marker). A red control gives this importer its own copy of either and shows a test
//! fail. Only the trust proof and the publish loop are here.
//!
//! **Decided before the first write.** [`plan_adoption`] runs under the import's locks, before any object is written: the
//! shared start, the signer's standing, and the chain's trust. A delivery that cannot be adopted is not refused for that
//! (adoption is opt-in; the history is still wanted): it lands under `remotes/` exactly as without `--adopt`, **with the reason
//! in the report and the typeable way forward**, never silently. [`publish_adoption`] runs after the writes and only does I/O.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload, SignerRole,
};

use super::adopt_local_ref::{
    AdoptionInputs, AdoptionStart, CurrentBranchMaterializeReport, Importer, begin_adoption,
    capture_for_current_branch, finish_current_branch_materialization,
};
use super::{BundleConnectivityClass, RefStateChainRelation};
use crate::RepositoryLayout;
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::object_store::ObjectReader;
use crate::refs::{RefPublication, RefStore};
use crate::trust::{
    GatedOperation, MaintainerTrustPolicy, verify_signer_trusted,
    verify_trusted_publication_envelope,
};

/// The bundle importer's name, for the shared adoption sentences.
pub(super) const BUNDLE_IMPORTER: Importer = Importer {
    command: "bundle-import",
    verb: "bundle import",
};

/// What the caller supplies to make `--adopt` possible: the receiver's own maintainer key (it signs the `RefUpdate`s) and the
/// current branch (adopting it also materializes it).
pub struct BundleAdoption<'a> {
    /// The receiver's maintainer signer. Its key must be adopted here (`verify_signer_trusted`), as for every publisher.
    pub signer: &'a dyn MaintainerSigner,
    /// The repository's current branch, if it has one (read by the caller, never by this crate).
    pub current_branch: Option<String>,
}

/// A [`MaintainerSigner`] over a `&dyn`, so `maintainer_signature` (which takes `&impl`) can sign with it.
struct DynSigner<'a>(&'a dyn MaintainerSigner);

impl MaintainerSigner for DynSigner<'_> {
    fn key_id(&self) -> &str {
        self.0.key_id()
    }
    fn sign(&self, preimage: &[u8]) -> Result<Vec<u8>> {
        self.0.sign(preimage)
    }
    fn public_key_bytes(&self) -> [u8; 32] {
        self.0.public_key_bytes()
    }
}

/// The decision [`plan_adoption`] reached, before any write.
pub(super) enum AdoptionDecision {
    /// `--adopt` was not asked for.
    NotRequested,
    /// `--adopt` was asked for and cannot land the branch: it lands under `remotes/` and this is why (typeable).
    FellThrough { reason: String },
    /// Adopt: the chain to publish, oldest first, and what the report says.
    Adopt(AdoptionPlan),
}

/// A trusted, complete RefState chain ready to publish.
pub(super) struct AdoptionPlan {
    /// The chain, oldest first: (RefState id, its payload).
    pub(super) states: Vec<(ObjectId, RefStatePayload)>,
    pub(super) remote_reason: Option<String>,
}

/// What an adopted import did, for the report.
pub(super) struct AdoptedOutcome {
    pub(super) materialization: Option<CurrentBranchMaterializeReport>,
}

/// Decide, before the first write, whether the delivered branch is adopted (see the module doc). Hard refusals (the queue owns
/// the name; a non-fast-forward without `--allow-non-fast-forward`; a signer this repository has not adopted) are `Err`;
/// everything else that stops adoption is a [`AdoptionDecision::FellThrough`] with a reason.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_adoption(
    layout: &RepositoryLayout,
    adoption: Option<&BundleAdoption<'_>>,
    origin_ref_name: &str,
    received_ref_name: &str,
    tip_ref_state_id: ObjectId,
    class: BundleConnectivityClass,
    chain: Option<RefStateChainRelation>,
    allow_non_fast_forward: bool,
    adopted_policy: &MaintainerTrustPolicy,
    reader: &impl ObjectReader,
) -> Result<AdoptionDecision> {
    let Some(adoption) = adoption else {
        return Ok(AdoptionDecision::NotRequested);
    };
    // Only branches are adopted: `heads/<name>` becomes a local branch; a tag or anything else stays a received ref.
    if !origin_ref_name.starts_with("heads/") {
        return Ok(AdoptionDecision::FellThrough {
            reason: format!(
                "{origin_ref_name} is not a branch (only a delivered heads/ ref is adopted); it is kept as {received_ref_name}"
            ),
        });
    }
    // The receiver signs the local ref log: its key must be adopted here, like every publisher's, and a refusal is before any write.
    verify_signer_trusted(
        layout,
        &DynSigner(adoption.signer),
        GatedOperation::BundleAdopt,
    )
    .map_err(|err| PrikkError::Precondition(err.to_string()))?;

    let ref_store = RefStore::new(layout.clone());
    let start = begin_adoption(
        &AdoptionInputs {
            layout,
            ref_store: &ref_store,
            importer: BUNDLE_IMPORTER,
            ref_name: origin_ref_name,
            destination: received_ref_name,
            allow_non_fast_forward,
        },
        || {
            // Over the metadata pass 1 already computed (the bundle importer's own classifier); the *decision* is shared.
            Ok((
                class,
                chain.ok_or_else(|| {
                    PrikkError::Integrity(
                        "bundle import --adopt: a received pointer exists but the delivery's chain relation was not computed"
                            .to_string(),
                    )
                })?,
            ))
        },
    )?;
    let remote_reason = match start {
        AdoptionStart::FellThrough { reason } => {
            return Ok(AdoptionDecision::FellThrough { reason });
        }
        AdoptionStart::Ready { remote_reason, .. } => remote_reason,
    };

    // The chain, newest to oldest, from the delivered tip: every step must be a held-or-carried RefState of this ref, signed by an
    // adopted key through the predicate `verify` uses; the oldest must have no predecessor (a ref is published from nothing).
    let mut newest_first: Vec<(ObjectId, RefStatePayload)> = Vec::new();
    let mut next = Some(tip_ref_state_id);
    while let Some(id) = next {
        let Some(envelope) = reader.read_object(id)? else {
            return Ok(AdoptionDecision::FellThrough {
                reason: format!(
                    "the delivered RefState chain is incomplete: RefState {id} is neither carried by the bundle nor held here, so \
                     {origin_ref_name} cannot be published from its first state; it is kept as {received_ref_name}"
                ),
            });
        };
        if envelope.object_type != ObjectType::RefState {
            return Ok(AdoptionDecision::FellThrough {
                reason: format!(
                    "{id} is a {}, not a RefState, so the chain cannot be adopted",
                    envelope.object_type
                ),
            });
        }
        if let Err(issue) = verify_trusted_publication_envelope(adopted_policy, &envelope) {
            return Ok(AdoptionDecision::FellThrough {
                reason: untrusted_reason(&issue, &envelope, id, received_ref_name),
            });
        }
        let payload = RefStatePayload::decode_canonical(
            &envelope.canonical_payload,
            envelope.schema_version,
        )?;
        if payload.ref_name != origin_ref_name || payload.kind != RefKind::Branch {
            return Ok(AdoptionDecision::FellThrough {
                reason: format!(
                    "RefState {id} is for {} ({:?}), not for the branch {origin_ref_name}; it is kept as {received_ref_name}",
                    payload.ref_name, payload.kind
                ),
            });
        }
        next = payload.previous_ref_state_id;
        newest_first.push((id, payload));
        if newest_first.len() > 1_000_000 {
            return Err(PrikkError::Integrity(
                "bundle import --adopt: the RefState chain is implausibly long".to_string(),
            ));
        }
    }
    newest_first.reverse();
    Ok(AdoptionDecision::Adopt(AdoptionPlan {
        states: newest_first,
        remote_reason,
    }))
}

/// The typeable reason a delivered RefState is not trusted: which key signed it, and what to do.
fn untrusted_reason(
    issue: &crate::trust::PublicationTrustIssue,
    envelope: &ObjectEnvelope,
    id: ObjectId,
    received_ref_name: &str,
) -> String {
    let keys: Vec<&str> = envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id.as_str())
        .collect();
    if issue.code == "PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE" {
        format!(
            "RefState {id} carries a MAINTAINER signature by an adopted key that does not verify; run `prikk verify` for the finding. \
             The history is kept as {received_ref_name}"
        )
    } else if keys.is_empty() {
        format!(
            "RefState {id} carries no MAINTAINER signature, so no adopted key vouches for it; it is kept as {received_ref_name}"
        )
    } else {
        format!(
            "RefState {id} is signed by {} and this repository has not adopted that key; adopt it with `prikk trust maintainer add \
             --key-id <id> --public-key <hex>` (the sender gives you the public key out of band), then run the import again with \
             --adopt. The history is kept as {received_ref_name}",
            keys.join(", ")
        )
    }
}

/// Publish the planned chain as the local branch, after the importer has written every carried object. The receiver signs one
/// `RefUpdate` per RefState, oldest first, over the delivered RefState (never re-signed). When the branch is the current branch it
/// is materialized into the worktree afterwards, by the shared capture and `materialize_after_publish`.
#[allow(clippy::too_many_arguments)]
pub(super) fn publish_adoption(
    layout: &RepositoryLayout,
    object_store: &mut crate::object_store::ObjectWriteSession,
    container_locks: &crate::lock::ContainerLockGuard,
    adoption: &BundleAdoption<'_>,
    plan: &AdoptionPlan,
    origin_ref_name: &str,
) -> Result<AdoptedOutcome> {
    let ref_store = RefStore::new(layout.clone());
    let final_state = plan.states.last().map(|(id, _)| *id).ok_or_else(|| {
        PrikkError::Integrity("bundle import --adopt: an empty RefState chain".to_string())
    })?;
    let pending = capture_for_current_branch(
        layout,
        adoption.current_branch.as_deref(),
        origin_ref_name,
        &*object_store,
        final_state,
        Some(
            "this bundle's own objects are already written; run the same import again to resume, picking up from here",
        ),
    )?;
    let signer = DynSigner(adoption.signer);
    let mut expected_previous: Option<ObjectId> = None;
    for (state_id, state) in &plan.states {
        let ref_state = object_store.read_object(*state_id)?.ok_or_else(|| {
            PrikkError::Integrity(format!(
                "bundle import --adopt: RefState {state_id} was not written by the import"
            ))
        })?;
        let update = RefUpdatePayload {
            ref_name: origin_ref_name.to_string(),
            old_ref_state_id: expected_previous,
            new_ref_state_id: *state_id,
            new_target_object_id: state.target_object_id,
            update_seq: state.update_seq,
            created_at: 0,
            author_key_id: signer.key_id().to_string(),
        };
        let mut update_envelope =
            ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
        let update_id = update_envelope.object_id();
        update_envelope.add_signature(maintainer_signature(
            &signer,
            ObjectType::RefUpdate,
            update_id,
        )?)?;
        ref_store.publish_with_object_store_under_held_lock(
            object_store,
            &RefPublication {
                ref_name: origin_ref_name.to_string(),
                expected_previous_ref_state_id: expected_previous,
                ref_state,
                ref_update: update_envelope,
            },
            container_locks,
        )?;
        expected_previous = Some(*state_id);
    }
    let materialization = pending
        .map(|pending| finish_current_branch_materialization(layout, pending))
        .transpose()?;
    Ok(AdoptedOutcome { materialization })
}
