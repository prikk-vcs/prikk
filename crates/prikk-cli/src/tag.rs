//! `prikk tag` — create and list tag refs (§6.6).
//!
//! Tags are immutable tag objects; a tag ref must point at the tag object, never directly at a
//! block — two hops: ref -> tag object -> block. Tag objects and tag ref-states are both
//! maintainer-signed, on the same terms as `seal`/`branch create`.
//!
//! `created_at` is the canonical no-clock sentinel (DC-34's "RefUpdate time policy", which
//! `TagPayload`'s corrected doc comment now also states): always zero, never derived from a wall
//! clock or accepted from a `--date` flag.
//!
//! Ref-name validation reuses `validate_local_tag_ref` (`prikk-store/src/refs.rs`), added
//! alongside the kind-aware branch in `validate_coherent_publication` — v1's AC4 finding that no
//! tag-appropriate validator existed. It mirrors `validate_local_branch_ref` with the prefix
//! requirement inverted and carries no case-collision rule, matching branches.

use std::path::PathBuf;

// RFC 121 §2.1: shadows the prelude's `println!`/`print!` -- see `crate::stdout`'s module doc.
use crate::arg_scan::{SetOnce, flag_value, mark_seen, unknown_argument};
use crate::commands::CliError;
use crate::stdout::println;
use prikk_object::{ObjectId, ObjectType, RefKind, RefStatePayload, TagPayload};
use prikk_store::{
    FileObjectStore, GatedOperation, ObjectReader, ObjectWriteSession, RefStore,
    compute_patch_set_digest_and_count_from_block, create_local_tag, validate_local_tag_ref,
    verify_signer_trusted,
};

/// Dispatch `prikk tag [list|create]`.
pub fn run_tag(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut iter = args.into_iter();
    let first = iter.next();
    match first.as_deref() {
        None | Some("list") => run_list(root, iter.collect()),
        Some("create") => run_create(root, iter.collect()),
        // RFC 146: no explicit subcommand keyword, matching `branch`'s own dispatch
        // (`branch.rs::run_branch`) -- a leading flag (e.g. bare `prikk tag --format json`) is an
        // argument to the implicit default, `list`, the same default `None` above already takes.
        Some(flag) if flag.starts_with('-') => {
            let mut rest = vec![flag.to_string()];
            rest.extend(iter);
            run_list(root, rest)
        }
        Some(other) => Err(CliError::Usage(format!(
            "unknown tag subcommand: {other} (expected list or create)"
        ))),
    }
}

fn run_list(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let mut format_json = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            // RFC 146: the same restricted-value pattern `worktree-status --format json` already
            // uses -- the only supported value is `json`.
            "--format" => {
                let value = flag_value(&mut iter, "tag list --format")?;
                if value != "json" {
                    return Err(CliError::Usage(format!(
                        "tag list --format does not support {value:?}"
                    )));
                }
                mark_seen(&mut format_json, "--format")?;
            }
            other => return Err(unknown_argument("tag list", other)),
        }
    }
    let layout = crate::open_repository(root)?;
    let ref_store = RefStore::new(layout.clone());
    let object_store = FileObjectStore::new(layout);
    let entries = ref_store
        .list_ref_pointers()
        .map_err(|err| err.to_string())?;
    let mut tags = Vec::new();
    for entry in entries {
        let ref_state_envelope = object_store
            .read_typed(entry.ref_state_id, ObjectType::RefState)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                format!(
                    "ref {} RefState {} is missing",
                    entry.ref_name, entry.ref_state_id
                )
            })?;
        let ref_state_payload = RefStatePayload::decode_canonical(
            &ref_state_envelope.canonical_payload,
            ref_state_envelope.schema_version,
        )
        .map_err(|err| err.to_string())?;
        if ref_state_payload.kind != RefKind::Tag {
            continue;
        }
        let tag_envelope = object_store
            .read_typed(ref_state_payload.target_object_id, ObjectType::Tag)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                format!(
                    "tag ref {} targets missing tag object {}",
                    entry.ref_name, ref_state_payload.target_object_id
                )
            })?;
        let tag_payload = TagPayload::decode_canonical(&tag_envelope.canonical_payload)
            .map_err(|err| err.to_string())?;
        tags.push(crate::output::TagListEntry {
            ref_name: entry.ref_name,
            target_block_id: tag_payload.target_block_id,
        });
    }
    if format_json {
        crate::output::print_tag_list_json(&tags);
    } else {
        let mut printed_any = false;
        for tag in &tags {
            println!("{} {}", tag.ref_name, tag.target_block_id);
            printed_any = true;
        }
        if !printed_any {
            println!("no tags");
        }
    }
    Ok(())
}

fn run_create(root: PathBuf, args: Vec<String>) -> std::result::Result<(), CliError> {
    let parsed = parse_create_args(args)?;
    let layout = crate::open_repository(root)?;
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = ObjectWriteSession::open(&layout).map_err(|err| err.to_string())?;

    let target_block_id = resolve_target_block(&ref_store, &object_store, &parsed.target)?;
    // RFC 117 T1 §3 / T7 §9.2: the tag's global identity is the digest of the target block's own
    // patch closure, computed at creation -- "two repositories holding the same patches produce the
    // same value" is the whole reason the field exists. `patch_count` travels alongside it (one
    // traversal, not two) so a receiver can prune candidates by size before hashing.
    let (patch_set_digest, patch_count) =
        compute_patch_set_digest_and_count_from_block(&object_store, target_block_id)
            .map_err(|err| err.to_string())?;

    let signer = crate::maintainer_signer_from_env()?;
    // DC-11/DC-63 §4: tag signing is on the same terms as `seal`, which includes this gate --
    // reused verbatim, before any object or ref write.
    verify_signer_trusted(&layout, &signer, GatedOperation::TagCreate)
        .map_err(|err| err.to_string())?;
    // RFC 117 stage 3 §4: the ordinary tag-creation path, shared with `sync adopt-tag`
    // (`tag_travel::adopt_tag`) rather than each carrying its own copy of the write+publish shape.
    let created = create_local_tag(
        &layout,
        &mut object_store,
        &parsed.name,
        target_block_id,
        parsed.message,
        patch_set_digest,
        patch_count,
        &signer,
    )
    .map_err(|err| err.to_string())?;
    let canonical = validate_local_tag_ref(&parsed.name).map_err(|err| err.to_string())?;

    println!("created tag {canonical}");
    println!("tag object: {}", created.tag_object_id);
    println!("target block: {target_block_id}");
    println!("RefState: {}", created.ref_state_id);
    Ok(())
}

/// Resolve `--target`'s block, accepting either a raw 64-hex block id or a ref name whose current
/// target is a block.
fn resolve_target_block(
    ref_store: &RefStore,
    object_store: &impl ObjectReader,
    target: &str,
) -> std::result::Result<ObjectId, String> {
    if let Ok(block_id) = target.parse::<ObjectId>() {
        return object_store
            .read_typed(block_id, ObjectType::Block)
            .map_err(|err| err.to_string())?
            .map(|_| block_id)
            .ok_or_else(|| format!("--target block {block_id} does not exist"));
    }
    let target_ref_state_id = ref_store
        .read_current_ref_state_id(target)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("--target ref {target} does not resolve to a published ref"))?;
    let target_envelope = object_store
        .read_typed(target_ref_state_id, ObjectType::RefState)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            format!("--target ref {target} RefState {target_ref_state_id} is missing")
        })?;
    let target_payload = RefStatePayload::decode_canonical(
        &target_envelope.canonical_payload,
        target_envelope.schema_version,
    )
    .map_err(|err| err.to_string())?;
    if target_payload.ref_name != target {
        return Err(format!(
            "--target RefState name mismatch: expected {target}, got {}",
            target_payload.ref_name
        ));
    }
    if object_store
        .read_typed(target_payload.target_object_id, ObjectType::Block)
        .map_err(|err| err.to_string())?
        .is_none()
    {
        return Err(format!(
            "--target ref {target} targets missing block {}",
            target_payload.target_object_id
        ));
    }
    Ok(target_payload.target_object_id)
}

struct CreateArgs {
    name: String,
    target: String,
    message: Option<String>,
}

fn parse_create_args(args: Vec<String>) -> std::result::Result<CreateArgs, CliError> {
    let mut name = None;
    let mut target = None;
    let mut message = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                let value = flag_value(&mut iter, "tag create --target")?;
                target.set_once("--target", value)?;
            }
            "-m" | "--message" => {
                let value = flag_value(&mut iter, "tag create message option")?;
                message.set_once("-m/--message", value)?;
            }
            other if other.starts_with('-') => {
                return Err(unknown_argument("tag create", other));
            }
            _ => {
                if name.is_some() {
                    return Err(CliError::Usage(
                        "tag create accepts at most one name".to_string(),
                    ));
                }
                name = Some(arg);
            }
        }
    }
    let Some(name) = name else {
        return Err(CliError::Usage("tag create requires <name>".to_string()));
    };
    let Some(target) = target else {
        return Err(CliError::Usage(
            "tag create requires --target <ref|block>".to_string(),
        ));
    };
    if let Some(msg) = &message {
        if msg.trim().is_empty() {
            return Err(CliError::Usage("tag message must not be empty".to_string()));
        }
    }
    Ok(CreateArgs {
        name,
        target,
        message,
    })
}
