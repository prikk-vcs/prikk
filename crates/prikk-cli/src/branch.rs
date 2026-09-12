//! `prikk branch` — list, create, and close local branch refs.
//!
//! Branch creation already exists: `commit --ref heads/topic` followed by `seal --ref heads/topic`
//! creates an unborn branch as a signed Root block at `update_seq = 1` (DC-13 Non-Default Ref
//! Genesis). `branch create --from` must publish the *same* ref-state shape DC-13's genesis
//! publishes — maintainer-signed, `RefKind::Branch`, `update_seq = 1`,
//! `previous_ref_state_id = None` for a name with no surviving log — so a branch is
//! indistinguishable afterward regardless of which path created it. The only permitted
//! difference is the target: DC-13 seals a block it just created; this points at a block that
//! already exists.
//!
//! **`branch close` (DC-61) is closure, not deletion.** Nothing is removed: the pointer, its
//! history, and every object stay. Closure publishes a final ref state carrying the `closed`
//! field (`RefStatePayload` tag 7, schema 2). DC-60 tried deletion (removing the pointer while
//! keeping the log); that produced "pointer absent, log present," a state the system does not
//! merely classify as corruption but at the time had a *repair function* for
//! (`RefStore::recoverable_missing_ref` plus a since-superseded `doctor` reconstruction path,
//! removed by the dead-surface consolidation once DC-38's crash recovery made it unreachable), so
//! `doctor` would have offered to resurrect every deleted branch, and it also bricked
//! repository-wide commits at every record count
//! (`.git-exclude/reviewed/prikk-dc60-delete-divergence-ruling-v1.md`). Closure leaves the pointer
//! present, so `verify`, `publish`, `recoverable_missing_ref`, and `doctor` all take their ordinary
//! paths — no new state, no new arm, nothing to repair.
//!
//! **Reopening a closed branch is an ordinary CAS update** (publish a new ref state without the
//! `closed` field) and is permitted, but this increment does not add a `branch reopen` CLI verb —
//! the command surface DC-61 specifies is `close` and `list --all` only. The capability is
//! exercised directly against `RefStore::publish` in the test suite.
//!
//! No current-branch pointer/`switch`: see the module-level non-goal in the accepted RFC (DC-60).
//! Every command still resolves `--ref` exactly as before.

use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::stdout::println;
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload,
};
use prikk_store::{
    ActiveRefOwnership, DEFAULT_ACTIVE_NAME, DEFAULT_CHECKOUT_REF, FileObjectStore, GatedOperation,
    MaintainerSigner, ObjectReader, ObjectWriteSession, RefPublication, RefStore, Wal,
    active_ref_ownership, maintainer_signature, resolve_ref_tip_block, validate_local_branch_ref,
    verify_signer_trusted,
};

/// Envelope schema version for a `RefState` carrying no `closed` field (every ordinary
/// publication: create, seal-genesis, and reopening alike).
const REF_STATE_SCHEMA_OPEN: u32 = 1;
/// Envelope schema version for a `RefState` whose `closed` field is present (DC-61).
const REF_STATE_SCHEMA_CLOSED: u32 = 2;

/// Dispatch `prikk branch [list|create|close]`.
pub fn run_branch(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    let first = iter.next();
    match first.as_deref() {
        None | Some("list") => run_list(root, iter.collect()),
        Some("create") => run_create(root, iter.collect()),
        Some("close") => run_close(root, iter.collect()),
        // No explicit subcommand keyword: a leading flag (e.g. bare `prikk branch --all`) is an
        // argument to the implicit default, `list` — the same default `None` above already takes.
        Some(flag) if flag.starts_with('-') => {
            let mut rest = vec![flag.to_string()];
            rest.extend(iter);
            run_list(root, rest)
        }
        Some(other) => Err(CliError::Usage(format!(
            "unknown branch subcommand: {other} (expected list, create, or close)"
        ))),
    }
}

fn run_list(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut show_all = false;
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--all" => mark_seen(&mut show_all, "--all")?,
            // RFC 146: the same restricted-value pattern `worktree-status --format json` already
            // uses -- the only supported value is `json`.
            "--format" => {
                let value = flag_value(&mut iter, "branch list --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "branch list --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other => return Err(unknown_argument("branch list", other)),
        }
    }
    let layout = crate::open_repository(root)?;
    let ref_store = RefStore::new(layout.clone());
    let object_store = FileObjectStore::new(layout.clone());
    let entries = ref_store
        .list_ref_pointers()
        .map_err(|err| err.to_string())?;
    let mut branches = Vec::new();
    for entry in entries {
        let envelope = object_store
            .read_typed(entry.ref_state_id, ObjectType::RefState)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                format!(
                    "ref {} RefState {} is missing",
                    entry.ref_name, entry.ref_state_id
                )
            })?;
        let payload =
            RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
                .map_err(|err| err.to_string())?;
        // RFC 146 §8a: `list_ref_pointers` returns every kind, so without this a tag ref was
        // listed as a branch -- a display slip in prose, but in `branch-list-v1` structured data
        // asserting a tag *is* a branch, carrying a `closed` boolean that means nothing for one.
        // `tag.rs`'s own listing has filtered on `RefKind::Tag` since it was written; this is the
        // same check on the other side. No new decode: `kind` comes from the payload the `closed`
        // flag below already needed.
        if payload.kind != RefKind::Branch {
            continue;
        }
        if payload.closed && !show_all {
            continue;
        }
        branches.push(crate::output::BranchListEntry {
            ref_name: entry.ref_name,
            ref_state_id: entry.ref_state_id,
            closed: payload.closed,
        });
    }
    // Received refs (DC-78 ruling 4) live entirely outside refs/by-id/ and are never a local
    // branch — listed separately, never interleaved with the loop above, so a received ref can
    // never be mistaken for one this repository can seal to.
    let received: Vec<crate::output::ReceivedListEntry> =
        prikk_store::list_received_pointers(&layout)
            .map_err(|err| err.to_string())?
            .into_iter()
            .map(|pointer| crate::output::ReceivedListEntry {
                ref_name: pointer.ref_name,
                ref_state_id: pointer.ref_state_id,
            })
            .collect();
    if format_json {
        crate::output::print_branch_list_json(&branches, &received);
    } else {
        let mut printed_any = false;
        for branch in &branches {
            if branch.closed {
                println!("{} {} (closed)", branch.ref_name, branch.ref_state_id);
            } else {
                println!("{} {}", branch.ref_name, branch.ref_state_id);
            }
            printed_any = true;
        }
        for entry in &received {
            println!("{} {} (received)", entry.ref_name, entry.ref_state_id);
            printed_any = true;
        }
        if !printed_any {
            println!("no branches");
        }
    }
    Ok(())
}

fn run_create(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_create_args(args)?;
    let layout = crate::open_repository(root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    let canonical = validate_local_branch_ref(&parsed.name).map_err(|err| err.to_string())?;
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = ObjectWriteSession::open(&layout).map_err(|err| err.to_string())?;

    if ref_store
        .read_current_ref_state_id(&canonical)
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err(format!("branch {canonical} already exists").into());
    }

    // A ref log can survive an interrupted publication with no live pointer. Creating over it
    // would produce the "pointer absent, log present" state that DC-61 exists to resolve with a
    // ref-log tombstone; `publish` would refuse anyway, but fail closed here with a clear message
    // rather than a generic classification failure.
    if ref_store
        .recoverable_missing_ref(&canonical)
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err(format!(
            "branch {canonical} has a surviving ref log with no live pointer; resuming it is not \
             yet supported (see DC-61), and creating over it would produce a corrupt state"
        )
        .into());
    }

    let from_ref = parsed
        .from
        .unwrap_or_else(|| DEFAULT_CHECKOUT_REF.to_string());
    let target_object_id = resolve_published_target(&ref_store, &object_store, &from_ref)?;

    let signer = crate::maintainer_signer_from_env()?;
    // DC-11/DC-63 §4: publishing a maintainer-signed object is gated on the same terms as `seal`,
    // reused verbatim, before any object or ref write.
    verify_signer_trusted(&layout, &signer, GatedOperation::BranchCreate)
        .map_err(|err| err.to_string())?;
    let ref_state_payload = RefStatePayload {
        ref_name: canonical.clone(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        REF_STATE_SCHEMA_OPEN,
        ref_state_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        &signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: canonical.clone(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: target_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        &signer,
    )?;
    let publication = RefPublication {
        ref_name: canonical.clone(),
        expected_previous_ref_state_id: None,
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published_ref_state_id = ref_store
        .publish_with_object_store(&mut object_store, &publication)
        .map_err(|err| err.to_string())?;

    println!("created branch {canonical}");
    println!("target block: {target_object_id}");
    println!("RefState: {published_ref_state_id}");
    println!("update_seq: 1");
    Ok(())
}

fn run_close(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut name = None;
    for arg in args {
        if name.is_some() {
            return Err(CliError::Usage(format!(
                "branch close accepts at most one name, got extra: {arg}"
            )));
        }
        name = Some(arg);
    }
    let Some(name) = name else {
        return Err(CliError::Usage("branch close requires <name>".to_string()));
    };

    let layout = crate::open_repository(root)?;
    layout
        .require_current_format()
        .map_err(|err| err.to_string())?;
    let canonical = validate_local_branch_ref(&name).map_err(|err| err.to_string())?;
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = ObjectWriteSession::open(&layout).map_err(|err| err.to_string())?;

    let Some(current_ref_state_id) = ref_store
        .read_current_ref_state_id(&canonical)
        .map_err(|err| err.to_string())?
    else {
        return Err(format!("branch {canonical} does not exist").into());
    };
    let current_envelope = object_store
        .read_typed(current_ref_state_id, ObjectType::RefState)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("branch {canonical} RefState {current_ref_state_id} is missing"))?;
    let current_payload = RefStatePayload::decode_canonical(
        &current_envelope.canonical_payload,
        current_envelope.schema_version,
    )
    .map_err(|err| err.to_string())?;
    if current_payload.closed {
        return Err(format!("branch {canonical} is already closed").into());
    }

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .map_err(|err| err.to_string())?;
    // RFC 102 Stage 2: a WAL whose only record is damaged would otherwise read as
    // `replay.records.is_empty()`, letting close proceed as if no active WAL owns anything here.
    if replay.has_item_failure() {
        return Err(
            "active WAL has a damaged record; run doctor before closing a branch"
                .to_string()
                .into(),
        );
    }
    if !replay.records.is_empty() {
        match active_ref_ownership(&layout, &canonical) {
            Ok(ActiveRefOwnership::Owned) => {
                return Err(format!(
                    "cannot close {canonical}: it owns a non-empty active WAL; seal it before closing"
                )
                .into());
            }
            // Owned by a different ref: this branch's own active WAL is not implicated, so closing
            // it may proceed. RFC 132 part 1: this is an answer to an ownership question, carried
            // as a plain value on a two-variant local enum rather than filed under a broad error
            // variant, precisely so this match cannot also swallow some *other*, unrelated
            // precondition that later starts reaching this same call site.
            Ok(ActiveRefOwnership::OwnedByOther(_)) => {}
            // Missing or malformed active-ref metadata on a non-empty WAL is an integrity condition,
            // not evidence this branch is uninvolved — fail closed like every other publisher
            // (`node_authoring.rs` propagates the same error via `?`) rather than treat "unknown
            // owner" as "not this branch."
            Err(err) => return Err(err.to_string().into()),
        }
    }

    let next_seq = current_payload
        .update_seq
        .checked_add(1)
        .ok_or_else(|| "ref-state update sequence overflow".to_string())?;

    let signer = crate::maintainer_signer_from_env()?;
    // DC-11/DC-63 §4: publishing a maintainer-signed object is gated on the same terms as `seal`,
    // reused verbatim, before any object or ref write.
    verify_signer_trusted(&layout, &signer, GatedOperation::BranchClose)
        .map_err(|err| err.to_string())?;
    let ref_state_payload = RefStatePayload {
        ref_name: canonical.clone(),
        kind: current_payload.kind,
        target_object_id: current_payload.target_object_id,
        update_seq: next_seq,
        previous_ref_state_id: Some(current_ref_state_id),
        required_attestation_ids: current_payload.required_attestation_ids.clone(),
        closed: true,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        REF_STATE_SCHEMA_CLOSED,
        ref_state_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        &signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: canonical.clone(),
        old_ref_state_id: Some(current_ref_state_id),
        new_ref_state_id: ref_state_id,
        new_target_object_id: current_payload.target_object_id,
        update_seq: next_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload
            .to_canonical_bytes()
            .map_err(|err| err.to_string())?,
        &signer,
    )?;
    let publication = RefPublication {
        ref_name: canonical.clone(),
        expected_previous_ref_state_id: Some(current_ref_state_id),
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published_ref_state_id = ref_store
        .publish_with_object_store(&mut object_store, &publication)
        .map_err(|err| err.to_string())?;

    println!("closed branch {canonical}");
    println!("RefState: {published_ref_state_id}");
    println!(
        "nothing was reclaimed; the pointer, its history, and every object remain, and the branch is recoverable"
    );
    Ok(())
}

/// Resolve `--from`'s target block, requiring it to be a currently published ref.
fn resolve_published_target(
    ref_store: &RefStore,
    object_store: &impl ObjectReader,
    from_ref: &str,
) -> std::result::Result<ObjectId, String> {
    let from_ref_state_id = ref_store
        .read_current_ref_state_id(from_ref)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("--from ref {from_ref} does not resolve to a published ref"))?;
    let from_envelope = object_store
        .read_typed(from_ref_state_id, ObjectType::RefState)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("--from ref {from_ref} RefState {from_ref_state_id} is missing"))?;
    let from_payload = RefStatePayload::decode_canonical(
        &from_envelope.canonical_payload,
        from_envelope.schema_version,
    )
    .map_err(|err| err.to_string())?;
    if from_payload.ref_name != from_ref {
        return Err(format!(
            "--from RefState name mismatch: expected {from_ref}, got {}",
            from_payload.ref_name
        ));
    }
    // RFC 147 §3d: `--from <ref>` means "at the Block this ref names", so a tag ref dereferences
    // through its Tag object exactly as every read surface now does. Before this, a tag `--from`
    // reported `object type mismatch: expected block, got tag` from the existence check below --
    // the tag was being read *as* a Block. This is the shared resolver, not a seventh copy of the
    // two hops; it resolves and never validates, which is why the existence check stays, now
    // applied to the **resolved** Block rather than to whatever the RefState pointed at.
    let (target_block_id, _tag_envelope) =
        resolve_ref_tip_block(object_store, &from_payload).map_err(|err| err.to_string())?;
    if object_store
        .read_typed(target_block_id, ObjectType::Block)
        .map_err(|err| err.to_string())?
        .is_none()
    {
        return Err(format!(
            "--from ref {from_ref} targets missing block {target_block_id}"
        ));
    }
    Ok(target_block_id)
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> std::result::Result<ObjectEnvelope, String> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope
        .add_signature(
            maintainer_signature(signer, object_type, object_id).map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    Ok(envelope)
}

struct CreateArgs {
    name: String,
    from: Option<String>,
}

fn parse_create_args(args: Vec<String>) -> std::result::Result<CreateArgs, CliError> {
    let mut name = None;
    let mut from = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from" => {
                let value = flag_value(&mut iter, "branch create --from")?;
                from.set_once("--from", value)?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("branch create", other));
            }
            _ => {
                if name.is_some() {
                    return Err(CliError::Usage(
                        "branch create accepts at most one name".to_string(),
                    ));
                }
                name = Some(arg);
            }
        }
    }
    let Some(name) = name else {
        return Err(CliError::Usage("branch create requires <name>".to_string()));
    };
    Ok(CreateArgs { name, from })
}
