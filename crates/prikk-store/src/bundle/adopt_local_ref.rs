//! The halves of "adopt a delivered ref as a real local branch" that **both importers share** (0.57.0 S1b, the owner's ruling
//! A): `archive import --adopt` and `bundle import --adopt`. Only the middle differs -- how each proves the delivered history
//! is trusted, and the publish loop (the archive republishes the sender's own `RefUpdate`s; a bundle carries none, so the
//! receiver signs its own over the delivered, adopted-key-signed `RefState`s, the shape `ref complete` already publishes).
//!
//! What is shared, in this file, called by both:
//!
//! 1. [`begin_adoption`]: the ref must be **absent** locally; the active queue must **not own** the name; the received pointer
//!    the delivery would replace is **classified**, and a `rollback`/`diverged`/`does-not-connect` landing refuses unless
//!    `--allow-non-fast-forward` (naming the importer's command). The classifier itself is supplied by each caller (the archive
//!    classifies over its object store, the bundle over its pass-1 metadata); the *decision* is here.
//! 2. [`capture_for_current_branch`]: when the adopted ref is the current branch, the worktree must be clean, nothing may be in
//!    the way of the tip's files, and the switch marker is set, so [`materialize_after_publish`] can finish it.
//!
//! 3. [`finish_current_branch_materialization`] and the re-exported report type, so the bundle importer reaches the worktree layer
//!    only through this module.
//!
//! The bundle importer's tests hold each shared half through the bundle path (a dirty worktree, a rollback that needs the flag).

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, RefStatePayload};

use super::{BundleConnectivityClass, RefStateChainRelation};
use crate::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::refs::RefStore;
use crate::wal::Wal;
use crate::worktree_materialize::{
    PendingCurrentBranchMaterialization, mark_current_branch_switch_pending,
    materialize_after_publish, require_nothing_in_the_way_for_block,
    require_worktree_clean_and_capture,
};

/// What materializing an adopted current branch did. Re-exported here so the bundle importer reaches the worktree layer through
/// the one place that owns the capture, not by a second path of its own.
pub(crate) use crate::worktree_materialize::CurrentBranchMaterializeReport;

/// Shared half 3 (see the module doc): finish what [`capture_for_current_branch`] began, once the ref is published.
pub(crate) fn finish_current_branch_materialization(
    layout: &RepositoryLayout,
    pending: PendingCurrentBranchMaterialization,
) -> Result<CurrentBranchMaterializeReport> {
    materialize_after_publish(layout, pending)
}

/// Which importer is adopting, for the sentences only.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Importer {
    /// `"archive-import"` or `"bundle-import"`: the `command` of the refusal document.
    pub(crate) command: &'static str,
    /// `"archive import"` or `"bundle import"`.
    pub(crate) verb: &'static str,
}

/// What [`begin_adoption`] decided.
pub(crate) enum AdoptionStart {
    /// The ref cannot be adopted, and the delivery lands under `remotes/` instead, **with this reason, which is never silent**.
    FellThrough { reason: String },
    /// Adoption may proceed.
    Ready {
        /// Set only when adoption proceeds *despite* a non-fast-forward (`--allow-non-fast-forward`).
        remote_reason: Option<String>,
        /// The delivery's class against the received pointer (`NoLocalHistory` when none existed).
        class: BundleConnectivityClass,
        /// How the delivered chain relates to the received pointer's; `None` when none existed.
        chain: Option<RefStateChainRelation>,
    },
}

/// Everything [`begin_adoption`] reads.
pub(crate) struct AdoptionInputs<'a> {
    pub(crate) layout: &'a RepositoryLayout,
    pub(crate) ref_store: &'a RefStore,
    pub(crate) importer: Importer,
    /// The local branch name that would be created (`heads/main`).
    pub(crate) ref_name: &'a str,
    /// Where the delivery lands without adoption (`remotes/heads/main`).
    pub(crate) destination: &'a str,
    pub(crate) allow_non_fast_forward: bool,
}

/// Shared half 1 (see the module doc). `classify` is called only when a received pointer already exists at
/// `inputs.destination`, and gives (class, chain) of the delivery against it.
pub(crate) fn begin_adoption(
    inputs: &AdoptionInputs<'_>,
    classify: impl FnOnce() -> Result<(BundleConnectivityClass, RefStateChainRelation)>,
) -> Result<AdoptionStart> {
    let AdoptionInputs {
        layout,
        ref_store,
        importer,
        ref_name,
        destination,
        allow_non_fast_forward,
    } = inputs;

    // Condition: the ref is absent locally. A name that exists is not overwritten, and the way forward depends on whether the
    // two histories are related, which this function does not decide; it names both.
    if let Some(existing) = ref_store.read_current_ref_state_id(ref_name)? {
        return Ok(AdoptionStart::FellThrough {
            reason: format!(
                "a local branch {ref_name} already exists (at RefState {existing}), and adopting onto an existing ref is not \
                 done; the history is kept as {destination}. If the two histories share a root, bring it in with `prikk merge \
                 --from {destination}`; if they are unrelated, the only route today is a new repository: `prikk setup <new-dir>`, \
                 then `prikk {verb} --adopt` there",
                verb = importer.verb,
            ),
        });
    }

    // 0.56.0 step W3 (R4): never adopt a name the active WAL already holds queued, unsealed work for. `seal` first.
    crate::commit_boundary::active::refuse_if_active_queue_owns_ref(
        layout,
        ref_name,
        &format!("adopting {ref_name}"),
    )?;

    // The received pointer at the destination: adopting must not silently erase what this repository already received there.
    let mut remote_reason = None;
    let mut class = BundleConnectivityClass::NoLocalHistory;
    let mut chain = None;
    if crate::received::read_received_pointer(layout, destination)?.is_some() {
        let (found_class, found_chain) = classify()?;
        class = found_class;
        chain = Some(found_chain);
        if matches!(
            class,
            BundleConnectivityClass::Rollback
                | BundleConnectivityClass::Diverged
                | BundleConnectivityClass::DoesNotConnect
        ) {
            if !allow_non_fast_forward {
                return Err(PrikkError::BundleNonFastForward {
                    command: importer.command,
                    class: class.as_str(),
                    message: format!(
                        "{verb} --adopt: adopting {ref_name} would be a {class} against the received pointer already held at \
                         {destination} -- refusing rather than silently rewriting received history; pass \
                         --allow-non-fast-forward to adopt it anyway",
                        verb = importer.verb,
                        class = class.as_str(),
                    ),
                });
            }
            remote_reason = Some(format!(
                "adopted despite a {} against the received pointer already held at {destination} (--allow-non-fast-forward)",
                class.as_str()
            ));
        }
    }
    Ok(AdoptionStart::Ready {
        remote_reason,
        class,
        chain,
    })
}

/// Shared half 2a, the **pre-write** form: when `ref_name` is `current_branch`, refuse -- before the importer writes anything --
/// a worktree that is not clean against its record or that has files in the way of the tip's. Captures nothing durable (the
/// returned capture is dropped: a resume is a new process, and the authoritative capture runs again after the writes, in
/// [`capture_for_current_branch`]). `target_block_id` is the Block the adopted tip points at, read through `reader`, which may
/// be the importer's own overlay of the artifact on the local store.
pub(crate) fn refuse_current_branch_obstacles(
    layout: &RepositoryLayout,
    current_branch: Option<&str>,
    ref_name: &str,
    reader: &impl ObjectReader,
    target_block_id: ObjectId,
) -> Result<()> {
    let Some(branch) = current_branch.filter(|branch| *branch == ref_name) else {
        return Ok(());
    };
    let active_replay =
        Wal::for_layout(layout, crate::foundation::layout::DEFAULT_ACTIVE_NAME).replay()?;
    let pending = require_worktree_clean_and_capture(
        layout,
        branch,
        &active_replay,
        &format!("adopting {branch}"),
    )?;
    require_nothing_in_the_way_for_block(
        layout,
        &pending,
        reader,
        branch,
        target_block_id,
        &format!("adopting {branch}"),
        None,
    )
}

/// Shared half 2 (see the module doc): when `ref_name` is `current_branch`, require a clean worktree and nothing in the way of
/// the final tip's files, capture the old files, and set the switch marker; `None` when it is not the current branch.
///
/// `final_ref_state_id` is the last RefState the adoption will publish; its target is already a held Block (the importer
/// wrote every carried object before this runs). `backstop_note` replaces "nothing was written" in a refusal that comes after
/// the importer's own writes.
pub(crate) fn capture_for_current_branch(
    layout: &RepositoryLayout,
    current_branch: Option<&str>,
    ref_name: &str,
    reader: &impl ObjectReader,
    final_ref_state_id: ObjectId,
    backstop_note: Option<&str>,
) -> Result<Option<PendingCurrentBranchMaterialization>> {
    let Some(branch) = current_branch.filter(|branch| *branch == ref_name) else {
        return Ok(None);
    };
    let active_replay =
        Wal::for_layout(layout, crate::foundation::layout::DEFAULT_ACTIVE_NAME).replay()?;
    let pending = require_worktree_clean_and_capture(
        layout,
        branch,
        &active_replay,
        &format!("adopting {branch}"),
    )?;
    let target_envelope = reader.read_object(final_ref_state_id)?.ok_or_else(|| {
        PrikkError::Integrity(format!(
            "adopting {ref_name}: RefState {final_ref_state_id} was not written by the import -- refusing to adopt"
        ))
    })?;
    let target_state = RefStatePayload::decode_canonical(
        &target_envelope.canonical_payload,
        target_envelope.schema_version,
    )?;
    require_nothing_in_the_way_for_block(
        layout,
        &pending,
        reader,
        branch,
        target_state.target_object_id,
        &format!("adopting {branch}"),
        backstop_note,
    )?;
    mark_current_branch_switch_pending(layout, &pending)?;
    Ok(Some(pending))
}
