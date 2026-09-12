//! Shared test fixtures and cross-module test harnesses.
//!
//! **Reachable under `test-support` as well as `cfg(test)`** (RFC 149 §6b), because the operations
//! layer's tests move to `prikk-operations` and their fixtures cannot follow them. Only the
//! fixtures a moving test actually reaches are re-exported from `lib.rs`; the rest are `pub` for
//! uniformity within this module and dead in a feature-only build, which is what the `allow` below
//! states. Adding a name to `lib.rs`'s block is a decision, exactly as it is for the
//! operations-layer contract.

// Under `cfg(test)` every fixture here has a caller. In a `--features test-support` build with no
// tests compiled, only the re-exported ones do -- and a fixture nobody has needed cross-crate yet
// is not a defect.
#![cfg_attr(not(test), allow(dead_code))]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, CreateFile, CreateSymlink, EditText, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PATCH_MESSAGE_SCHEMA,
    PatchPayload, PatchPurpose, RefKind, RefStatePayload, RefUpdatePayload, RenamePath,
    ReplaceBinary, Signature, SignatureAlgorithm, SignerRole,
};

use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};
use prikk_object::{BlobKind, BlobPayload};

/// A signed `Patch` envelope with one create operation, for fixtures that only need a valid patch.
pub fn signed_patch_envelope() -> ObjectEnvelope {
    let blob_id = signed_patch_blob_envelope().object_id();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([0x51; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, bytes);
    assert!(envelope.add_signature(rollback_author_signature()).is_ok());
    envelope
}

/// The `Blob` a [`signed_patch_envelope`] refers to, signed.
pub fn signed_patch_blob_envelope() -> ObjectEnvelope {
    signed_text_blob_envelope(b"patch fixture\n")
}

/// RFC 123 §8: a schema-4 patch carrying `message`, otherwise identical in shape to
/// `signed_patch_envelope` above -- used to prove `history::load_ref_history`'s `patch_messages`
/// surfaces a real message, the counterpart to that function's own `message: None` (schema 1).
pub fn signed_patch_envelope_with_message(message: &str) -> ObjectEnvelope {
    let blob_id = signed_patch_blob_envelope().object_id();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([0x51; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: Some(message.to_string()),
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, PATCH_MESSAGE_SCHEMA, bytes);
    assert!(envelope.add_signature(rollback_author_signature()).is_ok());
    envelope
}

/// Return a supported rollback-marked Patch envelope for sealed-history classification tests.
pub fn rollback_patch_envelope() -> ObjectEnvelope {
    let blob_id = rollback_patch_blob_envelope().object_id();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "rollback.txt".to_string(),
                node_id: NodeId::from_bytes([0x73; 32]),
                blob_id,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::RollbackDraft,
        message: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, bytes);
    assert!(envelope.add_signature(rollback_author_signature()).is_ok());
    envelope
}

/// The `Blob` a rollback `Patch` refers to, signed.
pub fn rollback_patch_blob_envelope() -> ObjectEnvelope {
    signed_text_blob_envelope(b"rollback fixture\n")
}

fn signed_text_blob_envelope(content: &[u8]) -> ObjectEnvelope {
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let bytes = payload.to_canonical_bytes().unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

/// A signed `Block` with no patches -- the smallest publishable block.
pub fn signed_empty_block_envelope() -> ObjectEnvelope {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).unwrap_or(MerkleRoot([0_u8; 32])),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, bytes);
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

/// A signed `RefState` envelope naming `block_id` as the ref tip.
pub fn signed_ref_state_envelope(
    ref_name: &str,
    previous_ref_state_id: Option<ObjectId>,
    target_object_id: ObjectId,
    update_seq: u64,
) -> ObjectEnvelope {
    let payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, bytes);
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

/// A signed `RefUpdate` envelope recording one ref moving to `ref_state_id`.
pub fn signed_ref_update_envelope(
    ref_name: &str,
    old_ref_state_id: Option<ObjectId>,
    new_ref_state_id: ObjectId,
    new_target_object_id: ObjectId,
    update_seq: u64,
) -> ObjectEnvelope {
    let payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id,
        new_ref_state_id,
        new_target_object_id,
        update_seq,
        created_at: 0,
        author_key_id: "maintainer-key".to_string(),
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let bytes = payload_bytes.unwrap_or_default();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, bytes);
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

/// A deterministic `ObjectId` derived from `label`, for fixtures that need a stable id nothing stores.
pub fn sample_object_id(label: &str) -> ObjectId {
    ObjectId::from_canonical_payload(ObjectType::Blob, 1, label.as_bytes())
}

/// A syntactically valid signature that verifies against nothing.
pub fn dummy_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "author-key".to_string(),
        signature_bytes: vec![1; 64],
        created_at: 7,
        signer_role: SignerRole::Author,
    }
}

/// A rollback-purpose AUTHOR signature, for fixtures exercising the rollback authority rules.
pub fn rollback_author_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "rollback-author-key".to_string(),
        signature_bytes: vec![7; 64],
        created_at: 7,
        signer_role: SignerRole::Author,
    }
}

/// The retired rollback marker-key signature, kept so the refusal of it can still be tested.
pub fn legacy_rollback_marker_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "dev-placeholder-rollback-author".to_string(),
        signature_bytes: vec![9; 64],
        created_at: 9,
        signer_role: SignerRole::Author,
    }
}

/// A MAINTAINER-role fixture signature. Distinct from `maintainer_signing::maintainer_signature`,
/// which signs for real -- this one is a value, not a signer.
pub fn maintainer_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "maintainer-key".to_string(),
        signature_bytes: vec![5; 64],
        created_at: 8,
        signer_role: SignerRole::Maintainer,
    }
}

/// Create a FIFO at `path` for a negative-control test fixture, portable between Linux and macOS.
/// `rustix::fs::mkfifoat`/`mknodat` are gated `#[cfg(not(any(apple, ...)))]` in `rustix` 1.1.4's own
/// source (`src/fs/at.rs`) — genuinely absent on `apple`, discovered by DC-81 only through actually
/// cross-compiling test code with `--target x86_64-apple-darwin`, since no production
/// `DurabilityContract` method calls `mkfifoat` and DC-76's own primitive-availability check never
/// had reason to look at it. `mkfifo(3)` is declared directly via FFI rather than adding a
/// dependency: it is a stable POSIX libc symbol every Unix `std` build already links against, so this
/// three-call-site test helper needs no `ALLOWED_THIRD_PARTY` change.
#[cfg(target_os = "linux")]
pub(crate) fn create_fifo_for_test(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    rustix::fs::mkfifoat(rustix::fs::CWD, path, rustix::fs::Mode::from_raw_mode(mode))
        .map_err(std::io::Error::from)
}

/// `crates/prikk-store` is `#![forbid(unsafe_code)]`, so a raw FFI declaration for `mkfifo(3)` is not
/// an option here — shelling out to the `mkfifo(1)` utility (a standard part of every macOS install,
/// including GitHub-hosted `macos-latest` runners) needs neither `unsafe` nor a new dependency.
#[cfg(target_os = "macos")]
pub(crate) fn create_fifo_for_test(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    let status = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg(format!("{mode:o}"))
        .arg(path)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mkfifo exited with status {status}"
        )))
    }
}

/// DC-84: routed through `unique_suffix()` below, which is the only part that actually guarantees
/// collision-freedom under thread contention.
pub fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("prikk-pr014-{name}-{}", unique_suffix()));
    assert!(std::fs::create_dir_all(&path).is_ok());
    path
}

/// DC-84: renamed from `monotonic_suffix`, which was wrong — it returned a wall-clock nanosecond
/// timestamp with **no counter at all**, and the misleading name caused a real error (DC-83's
/// handoff cited this function, by name, as the correct pattern to mirror, without reading its
/// body; blindly mirroring it would have left DC-83's bug in place, since `process::id()` is
/// constant across every thread of one process and cannot distinguish two racing threads of the
/// same test binary). Confirmed empirically (DC-83): a barrier-synchronized stress test against the
/// bare timestamp produced real collisions under thread contention (214 in 128,000 samples), and
/// zero once the `fetch_add` sequence number below was added — that atomic increment, not the
/// timestamp or the process id, is what makes this genuinely unique.
fn unique_suffix() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    };
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

mod rename_history;
// Not in the test-support surface: no movable family's test reaches these two, so they stay
// crate-internal. RFC 149 §6b exposes what was measured, not the module. `cfg(test)` on the
// re-export, not just the module, because in a feature-only build their only consumers -- this
// crate's own tests -- are not compiled.
#[cfg(test)]
pub(crate) use rename_history::{
    publish_two_nodes_then_rename_cycle_block,
    publish_two_nodes_then_rename_onto_occupied_path_block,
};

mod snapshot_history;
pub use snapshot_history::publish_snapshot_then_patch_block;

/// Publish one block creating a text node and a second editing it, and return the sealed ids.
pub fn publish_text_create_then_edit_block(
    layout: &RepositoryLayout,
    old: &[u8],
    new: &[u8],
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x81; 32]);
    let old_blob = write_blob(&mut object_store, old)?;
    let span = crate::text_span::plan_authored_text_span(old, new, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("test edit is unchanged".to_string()))?;

    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id,
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: span.span_id,
                    old_span_hash: span.old_span_hash,
                    left_anchor_hash: span.left_anchor_hash,
                    right_anchor_hash: span.right_anchor_hash,
                    replacement_text: span.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: span.old_span_text,
                    left_anchor_len: Some(span.left_anchor_len),
                    right_anchor_len: Some(span.right_anchor_len),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
        patch_payload.to_canonical_bytes()?,
    );
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state_root = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        None,
        state_root,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// A binary blob, unlike [`write_blob`]'s always-`BlobKind::Text`.
pub(crate) fn write_binary_blob(
    store: &mut FileObjectStore,
    bytes: &[u8],
) -> prikk_error::Result<ObjectId> {
    let payload = BlobPayload::new(BlobKind::Binary, bytes.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

/// One `CreateFile` (binary), sealed in its own root block, then a second block replacing its
/// content with `ReplaceBinary`. Returns the final `new_blob_id`. RFC 143's own content-report
/// tests need this; it lives here rather than in `patch_replay/tests/content_report.rs` because
/// this module -- unlike that one -- is already `#[cfg(test)]`-excluded from RFC 130's coupling
/// gate at `lib.rs`'s own top level, so a fixture calling `derive_next_state_root` (a `block_state`
/// re-export) here never risks a spurious `patch_replay -> block_state` production edge.
pub(crate) fn publish_binary_create_then_replace(
    layout: &RepositoryLayout,
    old: &[u8],
    new: &[u8],
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x83; 32]);
    let old_blob = write_binary_blob(&mut object_store, old)?;

    let create_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "asset.bin".to_string(),
                node_id,
                blob_id: old_blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut create_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, create_payload.to_canonical_bytes()?);
    create_patch.add_signature(dummy_signature())?;
    let create_patch_id = object_store.write_object(&create_patch)?;
    let root_state = crate::derive_next_state_root(&object_store, None, &[create_patch_id])?;
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![create_patch_id],
        None,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let new_blob = write_binary_blob(&mut object_store, new)?;
    let replace_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ReplaceBinary(ReplaceBinary {
                node_id,
                old_blob_id: old_blob,
                new_blob_id: new_blob,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut replace_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, replace_payload.to_canonical_bytes()?);
    replace_patch.add_signature(dummy_signature())?;
    let replace_patch_id = object_store.write_object(&replace_patch)?;
    let next_state =
        crate::derive_next_state_root(&object_store, Some(root_block_id), &[replace_patch_id])?;
    let next_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![replace_patch_id],
        None,
        next_state,
    );
    let next_block_id = object_store.write_object(&next_block)?;

    let ref_store = RefStore::new(layout.clone());
    let root_ref_state = signed_ref_state_envelope("heads/main", None, root_block_id, 1);
    let root_ref_state_id = root_ref_state.object_id();
    let root_ref_update =
        signed_ref_update_envelope("heads/main", None, root_ref_state_id, root_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: root_ref_state,
        ref_update: root_ref_update,
    })?;
    let next_ref_state =
        signed_ref_state_envelope("heads/main", Some(root_ref_state_id), next_block_id, 2);
    let next_ref_state_id = next_ref_state.object_id();
    let next_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(root_ref_state_id),
        next_ref_state_id,
        next_block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(root_ref_state_id),
        ref_state: next_ref_state,
        ref_update: next_ref_update,
    })?;
    Ok(new_blob)
}

/// RFC 134 §8, §3.2 ("demonstrate, do not assume" a v1-`EditText`-history repository still
/// transports and verifies cleanly): `publish_text_create_then_edit_block`'s own pre-§8 shape,
/// kept alive here so a genuine v1 (schema 1, `dup_index`-positional identity) fixture can still be
/// built for that demonstration -- production authoring no longer emits this shape (see
/// `plan_authored_text_span_v1`'s own doc), so this is the only way left to construct one.
// Gated to match its only caller: `bundle::tests` is `#[cfg(all(test, target_os = "linux"))]`, so
// on macOS and Windows this helper has no consumer and `-D dead-code` (implied by `-D warnings`)
// refuses the crate. The dependency is on a pre-existing platform gate elsewhere, which is why the
// increment that added this carried no `#[cfg(target_os)]` of its own and still broke both
// non-Linux jobs.
//
// **Not in RFC 149 §6b's test-support surface, deliberately.** Exposing it under the feature means
// compiling `text_span::plan_authored_text_span_v1` there too, and that pulls `AuthoredTextSpanV1`,
// `left_anchor` and `right_anchor` behind it -- `text_span`'s v1 authoring internals, widened for one
// Linux-only test of `bundle`. That trade is `bundle`'s move commit's to make, not this one's.
#[cfg(all(test, target_os = "linux"))]
pub fn publish_text_create_then_edit_block_v1(
    layout: &RepositoryLayout,
    old: &[u8],
    new: &[u8],
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x81; 32]);
    let old_blob = write_blob(&mut object_store, old)?;
    let span = crate::text_span::plan_authored_text_span_v1(old, new, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("test edit is unchanged".to_string()))?;

    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id,
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: span.span_id,
                    old_span_hash: span.old_span_hash,
                    left_anchor_hash: span.left_anchor_hash,
                    right_anchor_hash: span.right_anchor_hash,
                    replacement_text: span.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: span.old_span_text,
                    left_anchor_len: None,
                    right_anchor_len: None,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch_payload.to_canonical_bytes()?);
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state_root = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        None,
        state_root,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// `CreateFile`, then `EditText`, then `RenamePath` — one node, one path change, sealed via the
/// raw-patch-then-seal technique since `commit` never authors a `RenamePath` (that stays true
/// through RFC 144 increment 3). RFC 144 increment 1 made `patch_replay`'s own apply path accept
/// this; `patch_inverse` still refuses it independently (its own `RenamePath` arm, unrelated to
/// `patch_replay::decode::ensure_apply_supported`) pending a later increment's inverse-planning
/// support, which is why `patch_inverse`/`rollback_preview`/`rollback_draft`'s own
/// fails-closed-on-unsupported-operation tests still use this fixture and still pass.
pub fn publish_text_edit_then_rename_path_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x82; 32]);
    let old = b"alpha beta\n";
    let new = b"alpha BETA\n";
    let old_blob = write_blob(&mut object_store, old)?;
    let span = crate::text_span::plan_authored_text_span(old, new, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("test edit is unchanged".to_string()))?;

    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id,
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: span.span_id,
                    old_span_hash: span.old_span_hash,
                    left_anchor_hash: span.left_anchor_hash,
                    right_anchor_hash: span.right_anchor_hash,
                    replacement_text: span.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: span.old_span_text,
                    left_anchor_len: Some(span.left_anchor_len),
                    right_anchor_len: Some(span.right_anchor_len),
                }),
            },
            Operation {
                op_seq: 3,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id,
                    old_path: "README.md".to_string(),
                    new_path: "README2.md".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
        patch_payload.to_canonical_bytes()?,
    );
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state_root = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        None,
        state_root,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// `CreateFile`, then `EditText`, then `CreateSymlink` — unlike `RenamePath`, `CreateSymlink`
/// remains apply-unsupported after RFC 144 increment 1 (handoff §3: refused for a different
/// reason, no authoring path, out of this increment's scope). Kept as the "still genuinely
/// unsupported" fixture so `patch_replay`'s own fails-closed-on-unsupported-operation test keeps
/// probing a real refusal rather than one this increment removed.
pub(crate) fn publish_text_edit_then_unsupported_create_symlink_block(
    layout: &RepositoryLayout,
) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x82; 32]);
    let old = b"alpha beta\n";
    let new = b"alpha BETA\n";
    let old_blob = write_blob(&mut object_store, old)?;
    let span = crate::text_span::plan_authored_text_span(old, new, node_id)
        .map_err(|err| prikk_error::PrikkError::Integrity(err.to_string()))?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("test edit is unchanged".to_string()))?;

    let patch_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "README.md".to_string(),
                    node_id,
                    blob_id: old_blob,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::EditText(EditText {
                    node_id,
                    span_id: span.span_id,
                    old_span_hash: span.old_span_hash,
                    left_anchor_hash: span.left_anchor_hash,
                    right_anchor_hash: span.right_anchor_hash,
                    replacement_text: span.replacement_text,
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: span.old_span_text,
                    left_anchor_len: Some(span.left_anchor_len),
                    right_anchor_len: Some(span.right_anchor_len),
                }),
            },
            Operation {
                op_seq: 3,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateSymlink(CreateSymlink {
                    path: "link".to_string(),
                    node_id: NodeId::from_bytes([0x84; 32]),
                    target: "README.md".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
        patch_payload.to_canonical_bytes()?,
    );
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;
    let state_root = crate::derive_next_state_root(&object_store, None, &[patch_id])?;
    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        None,
        state_root,
    );
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// Write one `Blob` through the object store and return its id.
pub fn write_blob(
    store: &mut FileObjectStore,
    bytes: &[u8],
) -> prikk_error::Result<prikk_object::ObjectId> {
    let payload = BlobPayload::new(BlobKind::Text, bytes.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    store.write_object(&envelope)
}

/// A signed `Block` over the given patches, with the given parents.
pub fn signed_block(
    kind: BlockKind,
    parent_block_ids: Vec<prikk_object::ObjectId>,
    patch_ids: Vec<prikk_object::ObjectId>,
    snapshot_blob_ref: Option<prikk_object::ObjectId>,
) -> ObjectEnvelope {
    signed_block_with_state_root(
        kind,
        parent_block_ids,
        patch_ids,
        snapshot_blob_ref,
        crate::compute_state_root(&[]).unwrap_or(MerkleRoot([0_u8; 32])),
    )
}

/// A signed `Block` carrying an explicit state root, for fixtures that assert on it.
pub fn signed_block_with_state_root(
    kind: BlockKind,
    parent_block_ids: Vec<prikk_object::ObjectId>,
    patch_ids: Vec<prikk_object::ObjectId>,
    snapshot_blob_ref: Option<prikk_object::ObjectId>,
    state_merkle_root: MerkleRoot,
) -> ObjectEnvelope {
    let payload = BlockPayload {
        parent_block_ids,
        kind,
        patch_ids,
        state_merkle_root,
        snapshot_blob_ref,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes.unwrap_or_default());
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}

#[cfg(test)]
mod tests {
    //! DC-84: one demonstration for `unique_suffix()`, in the same shape DC-83 used to disprove the
    //! naive "process id plus timestamp" pattern — a synchronized-barrier thread-contention test.
    //! `unique_temp_dir` (used by all 580+ other tests in this crate) routes through this same
    //! function, so one demonstration here covers every caller.

    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn unique_suffix_has_no_collisions_under_synchronized_thread_contention() {
        let threads = 64;
        let rounds = 200;
        let mut total_samples = 0usize;
        let mut all_unique = HashSet::new();
        for _ in 0..rounds {
            let barrier = Arc::new(Barrier::new(threads));
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        super::unique_suffix()
                    })
                })
                .collect();
            for handle in handles {
                let value = match handle.join() {
                    Ok(value) => value,
                    Err(_) => panic!("stress-test thread panicked"),
                };
                total_samples += 1;
                assert!(
                    all_unique.insert(value.clone()),
                    "unique_suffix() produced a duplicate value under synchronized contention: {value}"
                );
            }
        }
        assert_eq!(all_unique.len(), total_samples);
    }
}
