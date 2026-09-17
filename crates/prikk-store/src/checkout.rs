//! Checkout planning helpers.
//!
//! PR-017 keeps a read-only checkout planning. It validates the current ref-state target and the
//! referenced block/patch objects, then reports what a future materializer would need to do. It
//! deliberately does not write the worktree and does not apply patch algebra.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, ObjectId, ObjectType, RefStatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::ref_resolution::{Point, PointKind};
use crate::refs::{RefStore, resolve_ref_tip_block};
use crate::snapshot::{SnapshotFile, load_block_snapshot};

/// Default ref used by checkout planning.
pub const DEFAULT_CHECKOUT_REF: &str = "heads/main";

/// Read-only plan for a future checkout/materialization operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutPlan {
    /// Human-readable ref name.
    pub ref_name: String,
    /// Current RefState ID, if the ref is published.
    pub ref_state_id: Option<ObjectId>,
    /// Target block ID, if the ref is published.
    pub block_id: Option<ObjectId>,
    /// Target block kind, if the target block exists and decodes.
    pub block_kind: Option<BlockKind>,
    /// Number of parent blocks referenced by the target block.
    pub parent_count: usize,
    /// Number of patches referenced by the target block.
    pub patch_count: usize,
    /// Optional snapshot blob reference from the target block.
    pub snapshot_blob_ref: Option<ObjectId>,
    /// Materialization status for this implementation stage.
    pub materialization: CheckoutMaterialization,
}

impl CheckoutPlan {
    /// Return true when the plan has a published target block.
    #[must_use]
    pub const fn has_target_block(&self) -> bool {
        self.block_id.is_some()
    }
}

/// Read-only plan for validating a snapshot-backed checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCheckoutPlan {
    /// Base checkout plan.
    pub checkout: CheckoutPlan,
    /// Snapshot Blob object ID.
    pub snapshot_blob_id: ObjectId,
    /// Number of files in the snapshot manifest.
    pub file_count: usize,
    /// Total content bytes in the snapshot manifest.
    pub total_content_bytes: u64,
    /// Validated repository-relative paths in materialization order.
    pub paths: Vec<String>,
}

/// What blocks a checkout from becoming a real worktree materialization in this stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutMaterialization {
    /// The requested ref is not published yet.
    UnpublishedRef,
    /// The target block has no patches and no snapshot to materialize.
    NoWorktreeChanges,
    /// A snapshot blob exists, but snapshot materialization is not implemented yet.
    RequiresSnapshotMaterialization,
    /// Patch application/algebra is required and is intentionally deferred.
    RequiresPatchEngine,
}

impl CheckoutMaterialization {
    /// Human-readable status label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnpublishedRef => "unpublished-ref",
            Self::NoWorktreeChanges => "no-worktree-changes",
            Self::RequiresSnapshotMaterialization => "requires-snapshot-materialization",
            Self::RequiresPatchEngine => "requires-patch-engine",
        }
    }
}

/// Prepare and validate a snapshot-backed checkout plan without writing the worktree.
pub fn prepare_snapshot_checkout_plan(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<SnapshotCheckoutPlan> {
    Ok(load_snapshot_checkout(layout, ref_name)?.0)
}

/// [`prepare_snapshot_checkout_plan`] together with the loaded files, for the materializer. The
/// snapshot is the target block's own state (RFC 136 §10.1a), loaded and checked by
/// `snapshot::load_block_snapshot` -- the same loader every replay reader uses.
pub(crate) fn load_snapshot_checkout(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(SnapshotCheckoutPlan, Vec<SnapshotFile>)> {
    let checkout = prepare_checkout_plan(layout, ref_name)?;
    load_snapshot_for_plan(layout, checkout)
}

/// [`prepare_snapshot_checkout_plan`] at a resolved [`Point`] -- a ref, or a bare block id (RFC 153
/// §7.1). Read-only.
///
/// # Errors
///
/// The point's block is not a checkpoint (`Precondition`), or its snapshot does not load (`Integrity`).
pub fn prepare_snapshot_checkout_plan_at_point(
    layout: &RepositoryLayout,
    point: &Point,
) -> Result<SnapshotCheckoutPlan> {
    let checkout = prepare_checkout_plan_at_point(layout, point)?;
    Ok(load_snapshot_for_plan(layout, checkout)?.0)
}

fn load_snapshot_for_plan(
    layout: &RepositoryLayout,
    checkout: CheckoutPlan,
) -> Result<(SnapshotCheckoutPlan, Vec<SnapshotFile>)> {
    let ref_name = checkout.ref_name.as_str();
    // RFC 132 per-site: a block with **no** snapshot reference is by design, not damage. Only a
    // checkpoint carries one (RFC 136 §10.2: a ref's first block and every 64 blocks after), so most
    // blocks are in this state -- reporting the normal case as `integrity error:` would tell a user
    // their repository was broken when nothing was wrong. `Precondition`, and the message carries the
    // route that works instead of only the fact.
    let (Some(snapshot_blob_id), Some(block_id)) = (checkout.snapshot_blob_ref, checkout.block_id)
    else {
        return Err(PrikkError::Precondition(format!(
            "checkout target for {ref_name} is not a checkpoint, so it carries no snapshot \
             (checkpoints fall at a ref's first block and every 64 blocks after); use `prikk \
             checkout --patch-plan --ref {ref_name}`, which replays without one"
        )));
    };
    let object_store = ObjectReadSnapshot::open(layout)?;
    // **The loader's refusals stay `Integrity`, deliberately.** A block that *references* a
    // snapshot Blob which is not there, or one that does not recompute to the block's state root, is
    // a different fact from a block with no snapshot at all: something wrote the reference and it
    // does not hold. That is damage, and the two must not be reclassified together.
    let block = load_block(&object_store, block_id)?;
    let files = load_block_snapshot(&object_store, block_id, &block)?.ok_or_else(|| {
        PrikkError::Integrity(format!(
            "checkout target Block {block_id} no longer references snapshot Blob {snapshot_blob_id}"
        ))
    })?;
    let plan = SnapshotCheckoutPlan {
        checkout,
        snapshot_blob_id,
        file_count: files.len(),
        total_content_bytes: files.iter().map(|file| file.bytes.len() as u64).sum(),
        paths: files.iter().map(|file| file.path.clone()).collect(),
    };
    Ok((plan, files))
}

/// [`prepare_checkout_plan`] at a resolved [`Point`] (RFC 153 §7.1). A ref point is planned exactly as
/// its name is; a bare block id has no RefState, so `ref_state_id` is `None` and the block is the one the
/// id names, validated the same way.
///
/// # Errors
///
/// The block, or an object it references, is missing or does not decode.
pub fn prepare_checkout_plan_at_point(
    layout: &RepositoryLayout,
    point: &Point,
) -> Result<CheckoutPlan> {
    if point.kind != PointKind::Block {
        return prepare_checkout_plan(layout, &point.name);
    }
    let object_store = ObjectReadSnapshot::open(layout)?;
    let block = load_block(&object_store, point.block_id)?;
    validate_block_references(&object_store, &block)?;
    let materialization = materialization_status(&block);
    Ok(CheckoutPlan {
        ref_name: point.name.clone(),
        ref_state_id: None,
        block_id: Some(point.block_id),
        block_kind: Some(block.kind),
        parent_count: block.parent_block_ids.len(),
        patch_count: block.patch_ids.len(),
        snapshot_blob_ref: block.snapshot_blob_ref,
        materialization,
    })
}

/// Prepare a checkout plan for a ref without modifying the worktree.
pub fn prepare_checkout_plan(layout: &RepositoryLayout, ref_name: &str) -> Result<CheckoutPlan> {
    let ref_store = RefStore::new(layout.clone());
    let object_store = ObjectReadSnapshot::open(layout)?;
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        return Ok(CheckoutPlan {
            ref_name: ref_name.to_string(),
            ref_state_id: None,
            block_id: None,
            block_kind: None,
            parent_count: 0,
            patch_count: 0,
            snapshot_blob_ref: None,
            materialization: CheckoutMaterialization::UnpublishedRef,
        });
    };

    let ref_state = load_ref_state(&object_store, ref_state_id, ref_name)?;
    // RFC 147 §3b: a `Tag` ref-state names a Tag object, not a Block -- one hop away. Resolved
    // through the same `refs::resolve_ref_tip_block` `bundle`, `patch_set_digest` and
    // `patch_exchange` already use, rather than a fourth copy of the two-hop walk. It resolves and
    // never validates (see its own doc); `validate_block_references` below is unchanged and still
    // does that job.
    let (block_id, _tag_envelope) = resolve_ref_tip_block(&object_store, &ref_state)?;
    let block = load_block(&object_store, block_id)?;
    validate_block_references(&object_store, &block)?;
    let materialization = materialization_status(&block);
    Ok(CheckoutPlan {
        ref_name: ref_name.to_string(),
        ref_state_id: Some(ref_state_id),
        block_id: Some(block_id),
        block_kind: Some(block.kind),
        parent_count: block.parent_block_ids.len(),
        patch_count: block.patch_ids.len(),
        snapshot_blob_ref: block.snapshot_blob_ref,
        materialization,
    })
}

fn load_ref_state(
    object_store: &impl ObjectReader,
    ref_state_id: ObjectId,
    ref_name: &str,
) -> Result<RefStatePayload> {
    let Some(envelope) = object_store.read_typed(ref_state_id, ObjectType::RefState)? else {
        return Err(PrikkError::Integrity(format!(
            "checkout ref {ref_name} points to missing RefState {ref_state_id}"
        )));
    };
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "checkout RefState name mismatch: expected {ref_name}, got {}",
            payload.ref_name
        )));
    }
    Ok(payload)
}

fn load_block(object_store: &impl ObjectReader, block_id: ObjectId) -> Result<BlockPayload> {
    let Some(envelope) = object_store.read_typed(block_id, ObjectType::Block)? else {
        return Err(PrikkError::Integrity(format!(
            "checkout target Block {block_id} is missing"
        )));
    };
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

fn validate_block_references(object_store: &impl ObjectReader, block: &BlockPayload) -> Result<()> {
    for parent in &block.parent_block_ids {
        if object_store
            .read_typed(*parent, ObjectType::Block)?
            .is_none()
        {
            return Err(PrikkError::Integrity(format!(
                "checkout target references missing parent Block {parent}"
            )));
        }
    }
    for patch in &block.patch_ids {
        if object_store
            .read_typed(*patch, ObjectType::Patch)?
            .is_none()
        {
            return Err(PrikkError::Integrity(format!(
                "checkout target references missing Patch {patch}"
            )));
        }
    }
    if let Some(snapshot) = block.snapshot_blob_ref {
        if object_store
            .read_typed(snapshot, ObjectType::Blob)?
            .is_none()
        {
            return Err(PrikkError::Integrity(format!(
                "checkout target references missing snapshot Blob {snapshot}"
            )));
        }
    }
    Ok(())
}

fn materialization_status(block: &BlockPayload) -> CheckoutMaterialization {
    if block.patch_ids.is_empty() && block.snapshot_blob_ref.is_none() {
        return CheckoutMaterialization::NoWorktreeChanges;
    }
    if block.snapshot_blob_ref.is_some() {
        return CheckoutMaterialization::RequiresSnapshotMaterialization;
    }
    CheckoutMaterialization::RequiresPatchEngine
}

#[cfg(test)]
mod tests;
