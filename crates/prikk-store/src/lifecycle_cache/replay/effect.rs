use super::*;

/// Apply one decoded operation's lifecycle state effect. `CreateFile`, `CreateSymlink`,
/// `DeleteNode`, `RenamePath`, `ChangePerm`, `ReplaceBinary`, and `EditText` are all exact as of
/// 2c-2d. All node-lifecycle apply failures map to `InconsistentLifecycleEffect`. `EditText`
/// additionally uses the materialized-text cache and the blob-content resolver.
pub(super) fn apply_state_effect<R: BlobKindResolver + BlobContentResolver>(
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    kind: &DecodedOperationKind,
    blob_resolver: &R,
) -> Result<(), LifecycleReplayError> {
    match kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            let repo_path = parse_repo_path(path)?;
            let blob_kind = blob_resolver
                .blob_kind(blob_id)
                .map_err(inconsistent)?
                .ok_or(LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id: *blob_id })?;
            let node_kind = NodeKind::from_file_blob_kind(blob_kind).map_err(inconsistent)?;
            let node = LiveNode {
                path: repo_path,
                kind: node_kind,
                content: NodeContent::File {
                    blob_id: *blob_id,
                    mode: *mode,
                },
            };
            state.create_node(*node_id, node).map_err(inconsistent)
        }
        DecodedOperationKind::CreateSymlink {
            path,
            node_id,
            target,
        } => {
            let repo_path = parse_repo_path(path)?;
            let node = LiveNode {
                path: repo_path,
                kind: NodeKind::Symlink,
                content: NodeContent::Symlink {
                    target: target.clone(),
                },
            };
            state.create_node(*node_id, node).map_err(inconsistent)
        }
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage,
        } => {
            // Exact replay (P1-1): the persisted record's old-state assertion (path, kind,
            // content) must match the replayed live node before the tombstone is recorded.
            let expected = expected_deleted_node(path, preimage)?;
            state
                .delete_node_checked(*node_id, &expected)
                .map(|_| ())
                .map_err(inconsistent)?;
            // The node's materialized text (if any) is no longer current.
            text_cache.remove(node_id);
            Ok(())
        }
        DecodedOperationKind::RenamePath { .. } => {
            // RFC 144 §4j / increment 2: a lone `RenamePath` applied here, one operation at a time,
            // is exactly the sequential fold that cannot represent a rename cycle (`rename_node`
            // rejects a target still occupied by its own swap partner). The caller is responsible
            // for collecting a run of consecutive `RenamePath` operations and resolving it through
            // `apply_rename_run` instead -- see that function and `collect_rename_run`. This arm
            // only says a lone rename is never handled *here*; it does not mean renames are
            // unsupported.
            Err(LifecycleReplayError::InconsistentLifecycleEffect {
                detail: "apply_state_effect received a RenamePath -- the caller must route \
                         consecutive RenamePath runs to apply_rename_run instead, never here"
                    .to_string(),
            })
        }
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => state
            .change_file_mode(*node_id, *old_mode, *new_mode)
            .map_err(inconsistent),
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            // Both blobs must be persisted and binary; the live node must currently reference
            // old_blob_id (checked in the substrate). Exact content swap, mode preserved.
            require_binary_blob(blob_resolver, *old_blob_id)?;
            require_binary_blob(blob_resolver, *new_blob_id)?;
            state
                .replace_file_blob(*node_id, *old_blob_id, *new_blob_id)
                .map_err(inconsistent)
        }
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
        } => apply_edit_text(
            state,
            text_cache,
            blob_resolver,
            *node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            *left_anchor_len,
            *right_anchor_len,
        ),
    }
}

/// Apply an `EditText` to the lifecycle index (2c-2d, forward only). Materializes the node's
/// current text (lazily, from the blob-content resolver), localizes the span per the FDD-01 §5.1
/// 64-byte anchor-filtered rule, splices in `replacement_text`, derives the new
/// `BlobPayload(Text, new_text)` object id, and records it as the node's content id. Mode,
/// `node_id`, and path are unchanged.
#[allow(clippy::too_many_arguments)]
fn apply_edit_text<R: BlobContentResolver>(
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    blob_resolver: &R,
    node_id: NodeId,
    span_id: &[u8; 32],
    old_span_hash: &[u8; 32],
    left_anchor_hash: &[u8; 32],
    right_anchor_hash: &[u8; 32],
    replacement_text: &[u8],
    old_span_text: &[u8],
    left_anchor_len: Option<u32>,
    right_anchor_len: Option<u32>,
) -> Result<(), LifecycleReplayError> {
    // Defense-in-depth: the canonical validator binds this at decode; re-assert here.
    if text_span_hash(old_span_text) != *old_span_hash {
        return Err(LifecycleReplayError::InconsistentLifecycleEffect {
            detail: "EditText old_span_hash != SHA-256(old_span_text)".to_string(),
        });
    }

    // Liveness + text-file eligibility; capture the current content blob id.
    let live = state.live_node(&node_id).ok_or_else(|| {
        LifecycleReplayError::InconsistentLifecycleEffect {
            detail: "EditText target node_id is not live".to_string(),
        }
    })?;
    if live.kind != NodeKind::TextFile {
        return Err(LifecycleReplayError::InconsistentLifecycleEffect {
            detail: "EditText target is not a text-file node".to_string(),
        });
    }
    let current_blob_id = match &live.content {
        NodeContent::File { blob_id, .. } => *blob_id,
        NodeContent::Symlink { .. } => {
            return Err(LifecycleReplayError::InconsistentLifecycleEffect {
                detail: "EditText target has symlink content".to_string(),
            });
        }
    };

    // Materialize current text: cache hit, else read the current content blob (must be Text).
    let current_text = match text_cache.get(&node_id) {
        Some(text) => text.clone(),
        None => {
            let (blob_kind, content) = blob_resolver
                .blob_content(&current_blob_id)
                .map_err(inconsistent)?
                .ok_or(LifecycleReplayError::MissingBlobForLifecycleEffect {
                    blob_id: current_blob_id,
                })?;
            if blob_kind != BlobKind::Text {
                return Err(LifecycleReplayError::InconsistentLifecycleEffect {
                    detail: "EditText current content blob is not Text".to_string(),
                });
            }
            content
        }
    };

    // Localize the span (FDD-01 §5.1, anchor-filtered) via the shared text-span module.
    let (start, end) = text_span::resolve_text_span(
        &current_text,
        old_span_text,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
        node_id,
        old_span_hash,
        left_anchor_len,
        right_anchor_len,
    )
    .map_err(|reason| LifecycleReplayError::TextSpanResolutionFailed {
        node_id,
        span_id: *span_id,
        reason,
    })?;

    // Splice and derive the new content identity through the shared module, so authoring and
    // replay produce the same bytes and the same `BlobPayload(Text, new_text)` id.
    let new_text = text_span::splice_text(&current_text, start, end, replacement_text)
        .map_err(inconsistent)?;
    let new_blob_id = text_span::text_blob_id(&new_text).map_err(inconsistent)?;
    state
        .set_text_blob(node_id, new_blob_id)
        .map_err(inconsistent)?;
    text_cache.insert(node_id, new_text);
    Ok(())
}

/// Extract one `RenamePath` operation's `(node_id, old_path, new_path)` triple, parsing both paths.
/// Callers only ever invoke this after already matching `DecodedOperationKind::RenamePath`.
fn rename_triple(
    kind: &DecodedOperationKind,
) -> Result<(NodeId, RepoPath, RepoPath), LifecycleReplayError> {
    let DecodedOperationKind::RenamePath {
        node_id,
        old_path,
        new_path,
    } = kind
    else {
        return Err(LifecycleReplayError::InconsistentLifecycleEffect {
            detail: "rename_triple called on a non-RenamePath operation".to_string(),
        });
    };
    Ok((
        *node_id,
        parse_repo_path(old_path)?,
        parse_repo_path(new_path)?,
    ))
}

/// Collect one run of consecutive `RenamePath` operations starting at `first` (already consumed
/// from `iter` by the caller), pulling further matching operations from `iter` via `next_if` so the
/// run stops at the first non-`RenamePath` operation without consuming it (RFC 144 §4j.3: a run is
/// scoped to consecutive same-kind operations within one patch, matching
/// `patch_replay::apply::apply_rename_batch`'s own scoping exactly). Shared by every fold site that
/// walks a patch's own operation list (§3 of the increment 2 handoff): `apply_patch_ids` and
/// `apply_queued_patch_envelopes` both call this the same way, so `replay_chain_with_appended_patches`
/// (which only ever reaches `apply_state_effect` through `apply_patch_ids`) inherits it too.
pub(super) fn collect_rename_run<'a>(
    iter: &mut std::iter::Peekable<
        std::slice::Iter<'a, crate::patch_replay::decode::DecodedPatchOperation>,
    >,
    first: &'a crate::patch_replay::decode::DecodedPatchOperation,
) -> Result<Vec<(NodeId, RepoPath, RepoPath)>, LifecycleReplayError> {
    let mut run = vec![rename_triple(&first.kind)?];
    while let Some(next) =
        iter.next_if(|operation| matches!(operation.kind, DecodedOperationKind::RenamePath { .. }))
    {
        run.push(rename_triple(&next.kind)?);
    }
    Ok(run)
}

/// Apply one collected rename run to `state` (RFC 144 §4j / increment 2) -- the same node-before-path
/// batch resolution [`crate::node::node_lifecycle::NodeLifecycleState::rename_nodes_checked_batch`]
/// implements, mapped into this module's own `LifecycleReplayError` taxonomy.
pub(super) fn apply_rename_run(
    state: &mut NodeLifecycleState,
    renames: &[(NodeId, RepoPath, RepoPath)],
) -> Result<(), LifecycleReplayError> {
    state
        .rename_nodes_checked_batch(renames)
        .map_err(inconsistent)
}

/// Require a blob to be present and `BlobKind::Binary` for a `ReplaceBinary` effect; a missing
/// blob is the fail-closed `MissingBlobForLifecycleEffect`, a non-binary blob is inconsistent.
fn require_binary_blob(
    resolver: &impl BlobKindResolver,
    blob_id: ObjectId,
) -> Result<(), LifecycleReplayError> {
    let kind = resolver
        .blob_kind(&blob_id)
        .map_err(inconsistent)?
        .ok_or(LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id })?;
    if kind != BlobKind::Binary {
        return Err(LifecycleReplayError::InconsistentLifecycleEffect {
            detail: format!("ReplaceBinary blob {blob_id} is not binary ({kind:?})"),
        });
    }
    Ok(())
}

/// Build the live node a `DeleteNode` record asserts it is deleting, from the persisted path and
/// discriminated deletion preimage, for exact-replay verification (P1-1).
fn expected_deleted_node(
    path: &str,
    preimage: &DecodedDeletePreimage,
) -> Result<LiveNode, LifecycleReplayError> {
    let repo_path = parse_repo_path(path)?;
    let (kind, content) = match preimage {
        DecodedDeletePreimage::File {
            old_node_kind,
            old_blob_id,
            old_mode,
        } => (
            *old_node_kind,
            NodeContent::File {
                blob_id: *old_blob_id,
                mode: *old_mode,
            },
        ),
        DecodedDeletePreimage::Symlink { old_target } => (
            NodeKind::Symlink,
            NodeContent::Symlink {
                target: old_target.clone(),
            },
        ),
    };
    Ok(LiveNode {
        path: repo_path,
        kind,
        content,
    })
}

/// Parse a decoded operation's raw path into a validated repo-relative path.
fn parse_repo_path(path: &str) -> Result<RepoPath, LifecycleReplayError> {
    RepoPath::parse(path).map_err(inconsistent)
}

/// Map a node-lifecycle apply error into the structured `InconsistentLifecycleEffect` class.
fn inconsistent<E: fmt::Display>(error: E) -> LifecycleReplayError {
    LifecycleReplayError::InconsistentLifecycleEffect {
        detail: error.to_string(),
    }
}
