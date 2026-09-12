//! Minimal local seal command implementation.
//!
//! This module publishes the currently active WAL as one Block and advances `heads/main` through
//! the existing RefState/RefUpdate primitives. It deliberately does not implement audit plugins,
//! patch application, real worktree state materialization, or conflict algebra.

use std::path::PathBuf;

use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload,
};
use prikk_store::{
    ActiveLock, ActiveRefMetadata, DEFAULT_ACTIVE_NAME, GatedOperation, MaintainerSigner,
    ObjectWriteSession, ObjectWriter, RefPublication, RefStore, RepositoryLayout, Wal,
    derive_next_state_root, finish_active_publication_cleanup, read_active_ref_metadata,
    remove_active_ref_metadata, validate_local_branch_ref, verify_signer_trusted,
};

mod support;

use support::{
    collect_wal_patch_ids, current_ref_state, current_tip_matches_wal_patches,
    finish_current_publication, persist_wal_patches, signed_envelope,
};

/// Result of sealing the current active WAL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealCommandResult {
    /// Ref advanced by seal.
    pub ref_name: String,
    /// Number of patch records sealed.
    pub patch_count: usize,
    /// New block ID.
    pub block_id: prikk_object::ObjectId,
    /// New RefState ID.
    pub ref_state_id: prikk_object::ObjectId,
}

/// Run the local seal scaffold against an already-parsed ref name. AUD-10: parsing is the caller's
/// job now (`parse_seal_args`, called from `main.rs` before the signer is built), so that a bad
/// argument is refused before an unrelated missing-signer environment is ever consulted.
pub fn run_seal(
    root: PathBuf,
    ref_name: Option<String>,
    signer: &impl MaintainerSigner,
) -> std::result::Result<SealCommandResult, CliError> {
    let layout = RepositoryLayout::open(root).map_err(|err| err.to_string())?;
    let ref_name = crate::current_branch::resolve_ref(&layout, ref_name)?;
    seal_active_no_audit(layout, &ref_name, signer).map_err(CliError::from)
}

/// Parse `prikk seal`'s own arguments. `pub(crate)` so `main.rs` can call it before building the
/// maintainer signer -- see [`run_seal`]'s doc comment.
pub(crate) fn parse_seal_args(args: Vec<String>) -> std::result::Result<Option<String>, CliError> {
    let mut allow_no_audit = false;
    let mut ref_name = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--allow-no-audit" => mark_seen(&mut allow_no_audit, "--allow-no-audit")?,
            "--ref" => {
                let value = flag_value(&mut iter, "seal --ref")?;
                ref_name.set_once("--ref", value)?;
            }
            other => return Err(unknown_argument("seal", other)),
        }
    }
    if !allow_no_audit {
        return Err(CliError::Usage(
            "seal scaffold requires --allow-no-audit".to_string(),
        ));
    }
    // RFC 151 §2.2: an explicit `--ref` is still validated here, before the signer is built; an
    // absent one resolves to the current branch once the repository is open (`run_seal`).
    ref_name
        .map(|ref_name| {
            validate_local_branch_ref(&ref_name).map_err(|err| CliError::from(err.to_string()))
        })
        .transpose()
}

fn seal_active_no_audit(
    layout: RepositoryLayout,
    ref_name: &str,
    signer: &impl MaintainerSigner,
) -> std::result::Result<SealCommandResult, String> {
    let ref_name = validate_local_branch_ref(ref_name).map_err(|err| err.to_string())?;
    let active_lock =
        ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).map_err(|err| err.to_string())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().map_err(|err| err.to_string())?;
    if replay.trailing_partial_bytes != 0 {
        return Err(format!(
            "active WAL has {} trailing partial bytes; run verify/doctor before seal",
            replay.trailing_partial_bytes
        ));
    }
    // RFC 102 Stage 2: a damaged record no longer makes `replay()` return `Err` -- without this,
    // `replay.records` below silently omits the damaged one, and `persist_wal_patches`/the sealed
    // Block's `patch_ids` would seal exactly that reduced set, permanently, with no refusal and no
    // trace of the dropped patch in history.
    if replay.has_item_failure() {
        return Err("active WAL has a damaged record; run verify/doctor before seal".to_string());
    }
    if replay.records.is_empty() {
        match read_active_ref_metadata(&layout).map_err(|err| err.to_string())? {
            ActiveRefMetadata::Missing => {}
            ActiveRefMetadata::Valid(_) | ActiveRefMetadata::Invalid(_) => {
                remove_active_ref_metadata(&layout).map_err(|err| err.to_string())?;
            }
        }
        return Err("active WAL has no patch records to seal".to_string());
    }
    match read_active_ref_metadata(&layout).map_err(|err| err.to_string())? {
        ActiveRefMetadata::Valid(actual) if actual == ref_name => {}
        ActiveRefMetadata::Valid(actual) => {
            return Err(format!(
                "active WAL is owned by {actual}; requested seal ref is {ref_name}"
            ));
        }
        ActiveRefMetadata::Missing => {
            return Err("active WAL has records but active ref metadata is missing".to_string());
        }
        ActiveRefMetadata::Invalid(reason) => {
            return Err(format!(
                "active WAL has records but active ref metadata is malformed: {reason}"
            ));
        }
    }

    let mut object_store = ObjectWriteSession::open(&layout).map_err(|err| err.to_string())?;
    let ref_store = RefStore::new(layout.clone());
    let current = current_ref_state(&layout, &object_store, &ref_store, &ref_name)?;
    let wal_patch_ids = collect_wal_patch_ids(&replay.records)?;
    if let Some(current) = current.as_ref() {
        if current_tip_matches_wal_patches(&object_store, current, &wal_patch_ids)? {
            verify_signer_trusted(&layout, signer, GatedOperation::Seal)
                .map_err(|err| err.to_string())?;
            finish_current_publication(
                &ref_store,
                &mut object_store,
                &active_lock,
                &ref_name,
                current,
                signer,
            )?;
            finish_active_publication_cleanup(&layout, &active_lock)
                .map_err(|err| err.to_string())?;
            return Ok(SealCommandResult {
                ref_name,
                patch_count: wal_patch_ids.len(),
                block_id: current.target_block_id,
                ref_state_id: current.ref_state_id,
            });
        }
    }
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    verify_signer_trusted(&layout, signer, GatedOperation::Seal).map_err(|err| err.to_string())?;
    let patch_ids = persist_wal_patches(&mut object_store, &replay.records)?;
    let parent = current.as_ref().map(|state| state.target_block_id);
    let state_merkle_root =
        derive_next_state_root(&object_store, parent, &patch_ids).map_err(|err| err.to_string())?;
    let parent_block_ids = parent.into_iter().collect();
    let block_payload = BlockPayload {
        parent_block_ids,
        kind: if current.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        patch_ids: patch_ids.clone(),
        state_merkle_root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_envelope = signed_envelope(
        ObjectType::Block,
        2,
        block_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        signer,
    )?;
    let block_id = object_store
        .write_object(&block_envelope)
        .map_err(|err| err.to_string())?;
    let update_seq = match current.as_ref() {
        Some(state) => state
            .update_seq
            .checked_add(1)
            .ok_or_else(|| format!("seal {ref_name}: ref-state update_seq overflow"))?,
        None => 1,
    };
    let previous_ref_state_id = current.as_ref().map(|state| state.ref_state_id);
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: previous_ref_state_id,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        signer,
    )?;
    let publication = RefPublication {
        ref_name: ref_name.clone(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published_ref_state_id = ref_store
        .finish_interrupted_publication_with_object_store(
            &mut object_store,
            &active_lock,
            &publication,
        )
        .map_err(|err| err.to_string())?;
    finish_active_publication_cleanup(&layout, &active_lock).map_err(|err| err.to_string())?;
    Ok(SealCommandResult {
        ref_name,
        patch_count: patch_ids.len(),
        block_id,
        ref_state_id: published_ref_state_id,
    })
}
