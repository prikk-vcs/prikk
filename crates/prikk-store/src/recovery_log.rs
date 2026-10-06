//! RFC 168 §3.1–§3.2: one append-only recovery log, `recovery/log`, and the way back from it.
//!
//! Every repair that removes bytes from a framed file appends an entry here **before** it truncates the file (C2). An
//! entry records the source file (relative to the repository root), the offset the file was cut at, the removed bytes,
//! the SHA-256 of the file's bytes before that offset (`prefix_hash`), and the identity of the **meaning files**: the
//! files whose state gives the removed bytes their meaning (RFC 168 §3.2, the table in [`meaning_paths_for`]).
//!
//! **The log is never truncated** except by [`clear`], which the user asks for. A damaged region is skipped byte-wise
//! to the next magic and counted, and the sound entries after it stay listed (the RFC 167 budget bounds the scan).
//! **The log is never authority:** no classification reads it, and `verify` reports damage in it on its own line.
//!
//! A restore (C3) writes the removed bytes back at exactly the recorded offset of exactly the recorded source, and only
//! when all three conditions hold: the source's length equals the offset, its prefix hashes to `prefix_hash`, and every
//! meaning file is unchanged. A source outside [`repairable_sources`] is refused whatever the entry says.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};

use crate::foundation::frame_resync::ScanBudget;
use crate::foundation::fsutil::{
    EntryKind, MutationRoot, append_file_required, create_new_file_required,
    ensure_directory_required, inspect_entry, list_directory, read_file_if_exists,
    read_file_range_if_exists, stat_file_state_if_exists, truncate_existing_file_required,
};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME, RepositoryLayout};

const MAGIC: &[u8; 8] = b"PRECLOG1";
const VERSION: u16 = 1;
/// Magic (8), version (2), body length (8), checksum (32).
const HEADER_LEN: usize = 8 + 2 + 8 + 32;
/// The log's path under `.prikk/`.
pub(crate) const LOG_PATH: &str = crate::foundation::layout::RECOVERY_LOG_RELATIVE;
/// The full entry id is the first 16 hex characters of the frame's checksum (RFC 168 §3.2).
pub(crate) const ID_LEN: usize = 16;

/// The identity of one file a removed span depends on: its path and the SHA-256 of its bytes, or `None` when it was
/// absent when the entry was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meaning {
    pub(crate) path: String,
    pub(crate) hash: Option<[u8; 32]>,
}

/// One saved repair: what it removed and everything a restore must check first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The source file, relative to the repository root, `/`-separated.
    pub(crate) source: String,
    /// The offset the source was cut at; the removed bytes start here.
    pub(crate) offset: u64,
    /// Which repair wrote the entry (for the listing and the plan).
    pub(crate) label: String,
    pub(crate) binary_version: String,
    /// SHA-256 of the source's bytes `[0, offset)` at repair time.
    pub(crate) prefix_hash: [u8; 32],
    pub(crate) meaning: Vec<Meaning>,
    pub(crate) removed: Vec<u8>,
}

/// What a repair reports about the entry it wrote: enough to name it on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRef {
    /// The 16-hex-character entry id.
    pub id: String,
    /// The source the entry saved bytes from, relative to the repository root.
    pub source: String,
    /// The offset the source was cut at.
    pub offset: u64,
    /// How many bytes were saved.
    pub len: u64,
}

impl std::fmt::Display for RecoveryRef {
    /// How a repair names its entry on screen: where it is, and how to read it back.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "recovery/log, entry {}", self.id)
    }
}

/// A decoded entry with its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) id: String,
    pub(crate) entry: Entry,
}

/// Everything `--recovery-list` and `verify` need from one read of the log.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub(crate) entries: Vec<Listed>,
    /// Damaged regions skipped by the resync. The log is never truncated, so they stay.
    pub(crate) damaged_regions: usize,
    /// True when the RFC 167 budget stopped the scan before the end of the log. Not "damage", but not fully read either.
    pub(crate) unread_tail: bool,
    /// True when unreadable bytes run to the end of the log with no sound entry after them: a save that was interrupted
    /// before its truncate. Harmless (C2: nothing was removed), so it is not counted as a damaged region.
    pub(crate) torn_tail: bool,
    /// The pre-RFC 168 `recovery/*.bytes` files, listed separately as "older format".
    pub(crate) older_files: Vec<String>,
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    prikk_hash::sha256(bytes)
}

fn put_str(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn encode_body(entry: &Entry) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, &entry.source);
    out.extend_from_slice(&entry.offset.to_be_bytes());
    put_str(&mut out, &entry.label);
    put_str(&mut out, &entry.binary_version);
    out.extend_from_slice(&entry.prefix_hash);
    out.extend_from_slice(&(entry.meaning.len() as u16).to_be_bytes());
    for meaning in &entry.meaning {
        put_str(&mut out, &meaning.path);
        match meaning.hash {
            Some(hash) => {
                out.push(1);
                out.extend_from_slice(&hash);
            }
            None => out.push(0),
        }
    }
    out.extend_from_slice(&(entry.removed.len() as u64).to_be_bytes());
    out.extend_from_slice(&entry.removed);
    out
}

/// The first [`ID_LEN`] hex characters of a checksum: an entry's id.
fn short_id(checksum: &[u8; 32]) -> String {
    prikk_hash::to_hex(checksum).chars().take(ID_LEN).collect()
}

fn checksum_of(body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(8 + 2 + 8 + body.len());
    preimage.extend_from_slice(MAGIC);
    preimage.extend_from_slice(&VERSION.to_be_bytes());
    preimage.extend_from_slice(&(body.len() as u64).to_be_bytes());
    preimage.extend_from_slice(body);
    sha(&preimage)
}

/// The framed bytes of one entry, and its id.
fn frame(entry: &Entry) -> (Vec<u8>, String) {
    let body = encode_body(entry);
    let checksum = checksum_of(&body);
    let id = short_id(&checksum);
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(body.len() as u64).to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    (out, id)
}

/// The id an entry would get, computed without writing anything. A plan-only preview names the same id a real repair
/// writes, because both come from the same frame.
pub(crate) fn id_of(entry: &Entry) -> String {
    frame(entry).1
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn hash(&mut self) -> Option<[u8; 32]> {
        self.take(32)?.try_into().ok()
    }
}

fn decode_body(body: &[u8]) -> Option<Entry> {
    let mut r = Reader { bytes: body, at: 0 };
    let source = r.str()?;
    let offset = r.u64()?;
    let label = r.str()?;
    let binary_version = r.str()?;
    let prefix_hash = r.hash()?;
    let count = r.u16()? as usize;
    // Grown as each meaning file decodes, never sized by the count: a body that claims more than it holds fails at `take`.
    let mut meaning = Vec::new();
    for _ in 0..count {
        let path = r.str()?;
        let hash = match r.take(1)?.first().copied()? {
            1 => Some(r.hash()?),
            0 => None,
            _ => return None,
        };
        meaning.push(Meaning { path, hash });
    }
    let n = usize::try_from(r.u64()?).ok()?;
    let removed = r.take(n)?.to_vec();
    if r.at != body.len() {
        return None;
    }
    Some(Entry {
        source,
        offset,
        label,
        binary_version,
        prefix_hash,
        meaning,
        removed,
    })
}

/// Append one entry and return its reference. The first save creates `recovery/log` (`init` creates it too, so in a
/// new repository the first save is an append). The caller holds the repair's locks.
pub(crate) fn append(root: &MutationRoot, entry: &Entry) -> Result<RecoveryRef> {
    let (framed, id) = frame(entry);
    ensure_directory_required(root, Path::new("recovery"))?;
    let path = Path::new(LOG_PATH);
    if read_file_range_if_exists(root, path, 0, 0)?.is_none() {
        // The log's first appearance is a new name: on Windows it is not durable (RFC 168 §3.1, residual (b)).
        create_new_file_required(root, path, &framed).map_err(|error| {
            PrikkError::MalformedData(format!("creating recovery/log: {error}"))
        })?;
    } else {
        append_file_required(root, path, &framed).map_err(|error| {
            PrikkError::MalformedData(format!("appending recovery/log: {error}"))
        })?;
    }
    Ok(RecoveryRef {
        id,
        source: entry.source.clone(),
        offset: entry.offset,
        len: entry.removed.len() as u64,
    })
}

/// Read every sound entry, skipping damaged regions byte-wise to the next magic (RFC 167 budget).
pub(crate) fn list(root: &MutationRoot) -> Result<Listing> {
    let mut listing = Listing {
        older_files: older_files(root)?,
        ..Listing::default()
    };
    let Some(bytes) = read_file_if_exists(root, Path::new(LOG_PATH))? else {
        return Ok(listing);
    };
    let mut budget = ScanBudget::for_input(bytes.len());
    let mut at = 0_usize;
    let mut in_damage = false;
    while at < bytes.len() {
        if budget.exceeded() {
            listing.unread_tail = true;
            break;
        }
        match parse_at(&bytes, at) {
            Some((listed_entry, next, charged)) => {
                budget.charge(charged);
                listing.entries.push(listed_entry);
                in_damage = false;
                at = next;
            }
            None => {
                budget.charge(1);
                if !in_damage {
                    listing.damaged_regions += 1;
                    in_damage = true;
                }
                at += 1;
            }
        }
    }
    if in_damage && !listing.unread_tail {
        // The unreadable run reached the end of the log: a torn tail, not a damaged region.
        listing.damaged_regions -= 1;
        listing.torn_tail = true;
    }
    Ok(listing)
}

/// Parse one framed entry at `at`: its listing, the offset after it, and the bytes charged to the budget.
fn parse_at(bytes: &[u8], at: usize) -> Option<(Listed, usize, u64)> {
    let header = bytes.get(at..at.checked_add(HEADER_LEN)?)?;
    if header.get(..8)? != MAGIC {
        return None;
    }
    let body_len =
        usize::try_from(u64::from_be_bytes(header.get(10..18)?.try_into().ok()?)).ok()?;
    let body_start = at.checked_add(HEADER_LEN)?;
    let body_end = body_start.checked_add(body_len)?;
    let body = bytes.get(body_start..body_end)?;
    let stored: [u8; 32] = header.get(18..50)?.try_into().ok()?;
    if checksum_of(body) != stored {
        return None;
    }
    let entry = decode_body(body)?;
    let id = short_id(&stored);
    Some((
        Listed { id, entry },
        body_end,
        (HEADER_LEN + body_len) as u64,
    ))
}

/// The pre-RFC 168 `recovery/*.bytes` files, by name.
fn older_files(root: &MutationRoot) -> Result<Vec<String>> {
    let recovery = Path::new("recovery");
    if inspect_entry(root, recovery)? != Some(EntryKind::Directory) {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in list_directory(root, recovery)? {
        if entry.kind == EntryKind::Regular {
            let name = entry.name.to_string_lossy().into_owned();
            if name.ends_with(".bytes") {
                names.push(format!("recovery/{name}"));
            }
        }
    }
    names.sort();
    Ok(names)
}

/// The fixed list of files a restore may write (RFC 168 Status: the allowlist). Derived from the layout, so it names the
/// live paths: both slots of every slot-based file, and the generation logs that select them.
pub(crate) fn repairable_sources(layout: &RepositoryLayout) -> Result<Vec<String>> {
    let mut paths: Vec<PathBuf> = vec![layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)];
    for slot in [ContainerSlot::A, ContainerSlot::B] {
        paths.push(layout.ref_pointer_index_slot_path(slot));
        paths.push(layout.ref_log_container_slot_path(slot));
        paths.push(layout.trust_policy_container_slot_path(slot));
        paths.push(layout.received_index_slot_path(slot));
    }
    paths.push(layout.ref_pointer_index_generation_log_path());
    paths.push(layout.trust_policy_generation_log_path());
    paths.push(layout.received_index_generation_log_path());
    paths.push(layout.trust_key_container_path());
    paths.push(layout.author_key_container_path());
    paths
        .iter()
        .map(|path| {
            Ok(layout
                .repository_relative(path)?
                .to_string_lossy()
                .replace('\\', "/"))
        })
        .collect()
}

/// **The meaning-file table (RFC 168 §3.2), one row per repair writer.** Each row names the files whose state gives the
/// removed bytes their meaning; their identities are recorded in the entry and checked before a restore. Derived from the
/// layout, so the same function serves the writer and the restore.
///
/// | source | meaning files | why (source) |
/// |---|---|---|
/// | the WAL (default session) | `ref-name`, the witness | the WAL's records belong to the ref in `ref-name` and are acknowledged by the witness (`commit_boundary/active.rs`, `witness.rs`) |
/// | the pointer index | the ref log's live slot | the pointer index names positions in the ref log it precedes (`refs/pointer_index.rs` header) |
/// | the ref log | the pointer index's live slot | the ref log's publications are what the pointer index names (`refs/pointer_index.rs:18-20`) |
/// | trust policy (either slot) | the trust policy generation log | the generation log selects the live slot (`foundation/generation.rs:407`) |
/// | received index (either slot) | the received index generation log | the same resolver |
/// | trust keys, author keys, the pointer-index, received-index and trust-policy generation logs | none | each file is read by its own frames; no other file changes what its bytes mean |
pub(crate) fn meaning_paths_for(layout: &RepositoryLayout, source: &str) -> Result<Vec<String>> {
    let relative = |path: &Path| -> Result<String> {
        Ok(layout
            .repository_relative(path)?
            .to_string_lossy()
            .replace('\\', "/"))
    };
    if source == relative(&layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME))? {
        return Ok(vec![
            relative(&layout.default_active_ref_name_path())?,
            relative(
                &layout
                    .active_session_dir(DEFAULT_ACTIVE_NAME)
                    .join("witness"),
            )?,
        ]);
    }
    let pointer_slot = resolve_live_slot(layout, &layout.ref_pointer_index_generation_log_path())?;
    if source == relative(&layout.ref_pointer_index_slot_path(pointer_slot))? {
        return Ok(vec![relative(
            &layout.ref_log_container_slot_path(ContainerSlot::A),
        )?]);
    }
    if source == relative(&layout.ref_log_container_slot_path(ContainerSlot::A))?
        || source == relative(&layout.ref_log_container_slot_path(ContainerSlot::B))?
    {
        return Ok(vec![relative(
            &layout.ref_pointer_index_slot_path(pointer_slot),
        )?]);
    }
    for slot in [ContainerSlot::A, ContainerSlot::B] {
        if source == relative(&layout.trust_policy_container_slot_path(slot))? {
            return Ok(vec![relative(&layout.trust_policy_generation_log_path())?]);
        }
        if source == relative(&layout.received_index_slot_path(slot))? {
            return Ok(vec![relative(
                &layout.received_index_generation_log_path(),
            )?]);
        }
    }
    Ok(Vec::new())
}

/// Hash the named meaning files as they are now.
pub(crate) fn meaning_now(root: &MutationRoot, paths: &[String]) -> Result<Vec<Meaning>> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("recovery-log-identity");
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let hash = read_file_if_exists(root, Path::new(path))?.map(|bytes| sha(&bytes));
        out.push(Meaning {
            path: path.clone(),
            hash,
        });
    }
    Ok(out)
}

/// Build an entry for a repair that cuts `source` (whose current bytes are `file_bytes`) at `cut`, saving what lies
/// beyond it. The meaning files come from [`meaning_paths_for`]. Pure apart from reading the meaning files.
pub(crate) fn entry_for(
    layout: &RepositoryLayout,
    source: &str,
    file_bytes: &[u8],
    cut: u64,
    label: &str,
) -> Result<Entry> {
    let cut_usize = usize::try_from(cut)
        .map_err(|_| PrikkError::MalformedData("repair offset does not fit usize".to_string()))?;
    let prefix = file_bytes
        .get(..cut_usize)
        .ok_or_else(|| PrikkError::MalformedData("repair offset beyond the file".to_string()))?;
    let removed = file_bytes.get(cut_usize..).unwrap_or_default().to_vec();
    let meaning = meaning_now(
        layout.repository_mutation_root(),
        &meaning_paths_for(layout, source)?,
    )?;
    Ok(Entry {
        source: source.to_string(),
        offset: cut,
        label: label.to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        prefix_hash: sha(prefix),
        meaning,
        removed,
    })
}

/// RFC 168 §3.1: record the object ids a rebuild of the object index lost (RFC 162 rule 2), sorted, one hex id per line. The
/// entry is not a byte cut: its source is the index, its removed bytes are the id list, and no restore writes it (the index is
/// rebuilt from its containers, which the plan names). Called before the rebuilt index is installed.
pub(crate) fn save_lost_ids(
    layout: &RepositoryLayout,
    lost_ids: &[prikk_object::ObjectId],
) -> Result<RecoveryRef> {
    let mut sorted = lost_ids.to_vec();
    sorted.sort();
    let mut contents = String::new();
    for id in &sorted {
        contents.push_str(&id.to_string());
        contents.push('\n');
    }
    let source = layout
        .repository_relative(&layout.container_index_path())?
        .to_string_lossy()
        .replace('\\', "/");
    let entry = lost_ids_entry(&source, contents.as_bytes());
    append(layout.repository_mutation_root(), &entry)
}

/// The entry for a lost-ids save (RFC 162 rule 2 via RFC 168 §3.1): not a byte cut, so the offset is 0 and the prefix is
/// empty. Its source is the object index, which no restore may write.
pub(crate) fn lost_ids_entry(source: &str, contents: &[u8]) -> Entry {
    Entry {
        source: source.to_string(),
        offset: 0,
        label: "object index lost ids".to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        prefix_hash: sha(&[]),
        meaning: Vec::new(),
        removed: contents.to_vec(),
    }
}

/// One restore condition and whether it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Condition {
    pub(crate) text: String,
    pub(crate) holds: bool,
}

/// What a restore would do, condition by condition (RFC 168 §3.2: the plan prints each condition, then what it writes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestorePlan {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) offset: u64,
    pub(crate) conditions: Vec<Condition>,
    /// The bytes that would be written at `offset`.
    pub(crate) would_write: Vec<u8>,
    /// Set when the entry's source is not on the allowlist, or the id names no entry.
    pub(crate) refusal: Option<String>,
    pub(crate) written: bool,
}

impl RestorePlan {
    /// Whether every condition holds and no refusal applies.
    pub(crate) fn can_restore(&self) -> bool {
        self.refusal.is_none() && self.conditions.iter().all(|condition| condition.holds)
    }
}

/// Plan, and when `plan_only` is false and every condition holds, write the removed bytes back at the recorded offset.
/// Writes nothing otherwise. The caller holds the repair's locks.
pub(crate) fn restore(layout: &RepositoryLayout, id: &str, plan_only: bool) -> Result<RestorePlan> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("recovery-log-identity");
    let root = layout.repository_mutation_root();
    if id.len() != ID_LEN || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(RestorePlan {
            id: id.to_string(),
            source: String::new(),
            offset: 0,
            conditions: Vec::new(),
            would_write: Vec::new(),
            refusal: Some(format!(
                "an entry id is {ID_LEN} hex characters; {id:?} is not one (`prikk doctor --recovery-list` prints the ids)"
            )),
            written: false,
        });
    }
    let listing = list(root)?;
    let Some(listed) = listing.entries.iter().find(|listed| listed.id == id) else {
        return Ok(RestorePlan {
            id: id.to_string(),
            source: String::new(),
            offset: 0,
            conditions: Vec::new(),
            would_write: Vec::new(),
            refusal: Some(format!(
                "no recovery entry has id {id}; run `prikk doctor --recovery-list` for the ids"
            )),
            written: false,
        });
    };
    let entry = &listed.entry;
    let mut plan = RestorePlan {
        id: id.to_string(),
        source: entry.source.clone(),
        offset: entry.offset,
        conditions: Vec::new(),
        would_write: entry.removed.clone(),
        refusal: None,
        written: false,
    };
    if !repairable_sources(layout)?.contains(&entry.source) {
        plan.refusal = Some(format!(
            "{} is not a file a restore may write: the object index is rebuilt from its containers \
             (`prikk doctor --repair-index`), and the other files on the repair list are the only \
             ones a restore writes",
            entry.source
        ));
        return Ok(plan);
    }
    let source_path = Path::new(&entry.source);
    let stat = stat_file_state_if_exists(root, source_path)?;
    let length_ok = stat.is_some_and(|state| state.size == entry.offset);
    plan.conditions.push(Condition {
        text: format!(
            "the source's length equals the recorded offset {}",
            entry.offset
        ),
        holds: length_ok,
    });
    let prefix_ok = match (
        length_ok,
        read_file_range_if_exists(
            root,
            source_path,
            0,
            usize::try_from(entry.offset).unwrap_or(usize::MAX),
        )?,
    ) {
        (true, Some(prefix)) => sha(&prefix) == entry.prefix_hash,
        _ => false,
    };
    plan.conditions.push(Condition {
        text: "the source's prefix hashes to the recorded prefix hash".to_string(),
        holds: prefix_ok,
    });
    for meaning in &entry.meaning {
        let now = read_file_if_exists(root, Path::new(&meaning.path))?.map(|bytes| sha(&bytes));
        plan.conditions.push(Condition {
            text: format!("the meaning file {} is unchanged", meaning.path),
            holds: now == meaning.hash,
        });
    }
    if plan.can_restore() && !plan_only {
        append_file_required(root, source_path, &entry.removed)?;
        plan.written = true;
    }
    Ok(plan)
}

/// The removed bytes of one entry, by id: for the repairs' own tests and for `verify`'s rehearsal reads.
#[cfg(test)]
pub(crate) fn removed_bytes(layout: &RepositoryLayout, id: &str) -> Result<Option<Vec<u8>>> {
    let listing = list(layout.repository_mutation_root())?;
    Ok(listing
        .entries
        .into_iter()
        .find(|listed| listed.id == id)
        .map(|listed| listed.entry.removed))
}

/// The one line `verify` prints about the log (RFC 168 §3.1), or `None` when the log is sound and fully read.
pub(crate) fn verify_line(layout: &RepositoryLayout) -> Result<Option<String>> {
    let listing = list(layout.repository_mutation_root())?;
    if listing.damaged_regions == 0 && !listing.unread_tail {
        return Ok(None);
    }
    Ok(Some(format!(
        "recovery log: {} damaged region{}{}; a save there cannot be restored (`prikk doctor --recovery-list`)",
        listing.damaged_regions,
        if listing.damaged_regions == 1 {
            ""
        } else {
            "s"
        },
        if listing.unread_tail {
            "; part of the log was not read within the scan budget"
        } else {
            ""
        },
    )))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::foundation::layout::RepositoryLayout;

    fn sample() -> Entry {
        Entry {
            source: "active/default/queue.wal".to_string(),
            offset: 12,
            label: "wal".to_string(),
            binary_version: "0.49.0".to_string(),
            prefix_hash: sha(b"prefix"),
            meaning: vec![Meaning {
                path: "active/default/ref-name".to_string(),
                hash: Some(sha(b"main")),
            }],
            removed: b"partial".to_vec(),
        }
    }

    #[test]
    fn an_entry_round_trips_through_its_frame_and_is_listed_under_its_id() {
        let entry = sample();
        let (framed, id) = frame(&entry);
        assert_eq!(id.len(), ID_LEN);
        let (listed, next, charged) = parse_at(&framed, 0).expect("a sound frame parses");
        assert_eq!(listed.entry, entry);
        assert_eq!(listed.id, id);
        assert_eq!(next, framed.len());
        assert_eq!(charged as usize, framed.len());
    }

    #[test]
    fn an_appended_entry_is_listed_under_the_id_append_returned() {
        let root = crate::test_gates::test_support::unique_temp_dir("recovery-log-append");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mutation = layout.repository_mutation_root();
        let reference = append(mutation, &sample()).expect("append");
        let listing = list(mutation).expect("list");
        assert_eq!(
            listing.entries.len(),
            1,
            "one entry listed after one append"
        );
        assert_eq!(listing.entries[0].id, reference.id);
        assert_eq!(listing.damaged_regions, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod controls {
    use super::*;
    use crate::foundation::layout::RepositoryLayout;
    use crate::test_gates::test_support::unique_temp_dir;

    fn entry(source: &str, removed: &[u8]) -> Entry {
        Entry {
            source: source.to_string(),
            offset: 4,
            label: "control".to_string(),
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            prefix_hash: sha(b"abcd"),
            meaning: Vec::new(),
            removed: removed.to_vec(),
        }
    }

    /// **Control: a damaged region in the log, with the entry after it still listed.** Garbage between two sound entries is
    /// counted as one damaged region, and the second entry is still read.
    #[test]
    fn a_damaged_region_is_counted_and_the_entry_after_it_stays_listed() {
        let root = unique_temp_dir("recovery-log-damaged-region");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mutation = layout.repository_mutation_root();
        let first = append(mutation, &entry("FORMAT", b"one")).expect("first");
        let path = root.join(".prikk").join(LOG_PATH);
        let mut bytes = std::fs::read(&path).expect("read log");
        bytes.extend_from_slice(b"damaged-garbage-between-entries");
        std::fs::write(&path, &bytes).expect("damage the log");
        let second = append(mutation, &entry("FORMAT", b"two")).expect("second");
        let listing = list(mutation).expect("list");
        assert_eq!(listing.damaged_regions, 1, "one damaged region");
        let ids: Vec<&str> = listing
            .entries
            .iter()
            .map(|listed| listed.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![first.id.as_str(), second.id.as_str()],
            "the entry after the damage is still listed"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A torn tail (a save interrupted before its truncate) is not damage: nothing was removed.
    #[test]
    fn a_torn_tail_of_the_log_is_not_counted_as_damage() {
        let root = unique_temp_dir("recovery-log-torn-tail");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mutation = layout.repository_mutation_root();
        append(mutation, &entry("FORMAT", b"one")).expect("append");
        let path = root.join(".prikk").join(LOG_PATH);
        let mut bytes = std::fs::read(&path).expect("read log");
        bytes.extend_from_slice(b"PRECLOG1-torn");
        std::fs::write(&path, &bytes).expect("tear the log");
        let listing = list(mutation).expect("list");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.damaged_regions, 0);
        assert!(listing.torn_tail);
        assert_eq!(verify_line(&layout).expect("line"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    /// **Control: a source outside the allowlist.** An entry naming `FORMAT` is refused whatever the entry says, and nothing
    /// is written.
    #[test]
    fn a_restore_refuses_a_source_outside_the_allowlist() {
        let root = unique_temp_dir("recovery-log-allowlist");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mutation = layout.repository_mutation_root();
        let format_before = std::fs::read(root.join(".prikk").join("FORMAT")).expect("FORMAT");
        let reference = append(mutation, &entry("FORMAT", b"6\n")).expect("append");
        let plan = restore(&layout, &reference.id, false).expect("plan");
        assert!(!plan.written);
        assert!(
            plan.refusal
                .as_deref()
                .is_some_and(|text| text.contains("not a file a restore may write")),
            "{plan:?}"
        );
        assert_eq!(
            std::fs::read(root.join(".prikk").join("FORMAT")).expect("FORMAT"),
            format_before
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// **Control: a wrong-length id.** Anything but the full 16 hex characters refuses.
    #[test]
    fn a_restore_refuses_an_id_that_is_not_sixteen_hex_characters() {
        let root = unique_temp_dir("recovery-log-id-length");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let reference =
            append(layout.repository_mutation_root(), &entry("FORMAT", b"x")).expect("append");
        for id in [&reference.id[..8], "zzzzzzzzzzzzzzzz", ""] {
            let plan = restore(&layout, id, true).expect("plan");
            assert!(!plan.written);
            assert!(plan.refusal.is_some(), "{id:?} must refuse");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}

/// One entry as `--recovery-list` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEntryView {
    /// The 16-hex-character id.
    pub id: String,
    /// The source the entry saved bytes from, relative to `.prikk/`.
    pub source: String,
    /// The offset the source was cut at.
    pub offset: u64,
    /// How many bytes were saved.
    pub len: u64,
    /// Which repair wrote the entry.
    pub label: String,
    /// The prikk version that wrote the entry.
    pub binary_version: String,
}

/// What `--recovery-list` prints: the sound entries, the damaged regions, and the older files listed by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryListing {
    /// The sound entries, in log order.
    pub entries: Vec<RecoveryEntryView>,
    /// Damaged regions the resync skipped (never truncated, so they stay).
    pub damaged_regions: usize,
    /// Unreadable bytes at the end of the log: an interrupted save, harmless.
    pub torn_tail: bool,
    /// The RFC 167 budget stopped the scan before the end of the log.
    pub unread_tail: bool,
    /// The `recovery/*.bytes` files a repair wrote before this format: listed, never restored, never deleted (RFC 168 C4).
    pub older_files: Vec<String>,
}

/// One condition of a restore plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreConditionView {
    /// What the condition checks, in the words the plan prints.
    pub text: String,
    /// Whether it holds now.
    pub holds: bool,
}

/// What `--recovery-restore` prints first: each condition, what it would write, and whether it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlanView {
    /// The entry id the plan is for.
    pub id: String,
    /// The file the restore would write to.
    pub source: String,
    /// The offset it would write at.
    pub offset: u64,
    /// Each condition and whether it holds.
    pub conditions: Vec<RestoreConditionView>,
    /// How many bytes the restore would write at `offset`.
    pub would_write: usize,
    /// The reason the restore cannot run at all (an unknown id, a malformed id, a source off the allowlist).
    pub refusal: Option<String>,
    /// Whether the bytes were written.
    pub written: bool,
}

/// What `--recovery-clear` removed, or would remove under `--plan-only`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryClearView {
    /// How many sound entries the log held.
    pub entries: usize,
    /// How many bytes the log held.
    pub bytes: usize,
    /// Whether the log was emptied (false under `--plan-only`).
    pub cleared: bool,
}

/// The locks a repair takes (`doctor::repair_tails`'s own set): restore and clear take the same ones, so neither runs
/// beside a repair or a writer of the files it may write.
fn repair_locks(
    layout: &RepositoryLayout,
) -> Result<(crate::lock::ActiveLock, crate::lock::ContainerLockGuard)> {
    use crate::foundation::layout::LockableContainer;
    let active = crate::lock::ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    let containers = crate::lock::acquire_container_locks(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
            LockableContainer::TrustPolicy,
        ],
    )?;
    Ok((active, containers))
}

/// `prikk doctor --recovery-list`: read the log. Judges no entry's restorability (that needs the source's whole hash, which a
/// listing does not pay for; `--recovery-restore --plan-only` judges one).
pub fn recovery_list(layout: &RepositoryLayout) -> Result<RecoveryListing> {
    let listing = list(layout.repository_mutation_root())?;
    Ok(RecoveryListing {
        entries: listing
            .entries
            .into_iter()
            .map(|listed| RecoveryEntryView {
                id: listed.id,
                source: listed.entry.source,
                offset: listed.entry.offset,
                len: listed.entry.removed.len() as u64,
                label: listed.entry.label,
                binary_version: listed.entry.binary_version,
            })
            .collect(),
        damaged_regions: listing.damaged_regions,
        torn_tail: listing.torn_tail,
        unread_tail: listing.unread_tail,
        older_files: listing.older_files,
    })
}

/// `prikk doctor --recovery-restore <id> [--plan-only]`: under the repair's locks, plan and, unless `plan_only`, write.
pub fn recovery_restore(
    layout: &RepositoryLayout,
    id: &str,
    plan_only: bool,
) -> Result<RestorePlanView> {
    let _locks = repair_locks(layout)?;
    let plan = restore(layout, id, plan_only)?;
    Ok(RestorePlanView {
        id: plan.id,
        source: plan.source,
        offset: plan.offset,
        conditions: plan
            .conditions
            .into_iter()
            .map(|condition| RestoreConditionView {
                text: condition.text,
                holds: condition.holds,
            })
            .collect(),
        would_write: plan.would_write.len(),
        refusal: plan.refusal,
        written: plan.written,
    })
}

/// `prikk doctor --recovery-clear [--plan-only]`: under the repair's locks, empty the log in place (the name is kept).
pub fn recovery_clear(layout: &RepositoryLayout, plan_only: bool) -> Result<RecoveryClearView> {
    let _locks = repair_locks(layout)?;
    let root = layout.repository_mutation_root();
    let listing = list(root)?;
    let bytes = read_file_if_exists(root, Path::new(LOG_PATH))?.map_or(0, |bytes| bytes.len());
    let mut cleared = false;
    if !plan_only && stat_file_state_if_exists(root, Path::new(LOG_PATH))?.is_some() {
        truncate_existing_file_required(root, Path::new(LOG_PATH), 0)?;
        cleared = true;
    }
    Ok(RecoveryClearView {
        entries: listing.entries.len(),
        bytes,
        cleared,
    })
}

/// The one line `verify` prints about the log, or `None` when the log is sound and fully read. Never changes `verify`'s exit
/// status (RFC 168 §3.1).
pub fn recovery_verify_line(layout: &RepositoryLayout) -> Result<Option<String>> {
    verify_line(layout)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod early_creation {
    use super::*;
    use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
    use crate::lock::ActiveLock;
    use crate::test_gates::test_support::unique_temp_dir;

    /// RFC 168 §3.1 and §3.3: `init` creates the log and the default session's empty witness.
    #[test]
    fn init_creates_the_log_and_the_default_witness_empty() {
        let root = unique_temp_dir("recovery-log-init");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        assert_eq!(
            std::fs::metadata(root.join(".prikk").join(LOG_PATH))
                .expect("log")
                .len(),
            0
        );
        let witness = layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness");
        assert_eq!(std::fs::metadata(witness).expect("witness").len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 §3.1 and §3.3: a repository created before the log and the witness existed gets them at its first write, and
    /// the lock taken for that write is released when it ends. An older `recovery/*.bytes` file is untouched.
    #[test]
    fn the_first_write_creates_a_missing_log_and_witness_and_leaves_older_files_alone() {
        let root = unique_temp_dir("recovery-log-first-write");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let witness = layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness");
        let log = root.join(".prikk").join(LOG_PATH);
        std::fs::remove_file(&log).expect("remove the log, as an older repository has none");
        std::fs::remove_file(&witness).expect("remove the witness");
        let older = root
            .join(".prikk")
            .join("recovery")
            .join("wal-default-at-3-0123456789abcdef.bytes");
        std::fs::write(&older, b"removed by a 0.48.0 repair").expect("an older file");

        {
            let _lock = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)
                .expect("the write takes the lock");
            assert!(log.is_file(), "the log is created at the first write");
            assert!(
                witness.is_file(),
                "the witness is created at the first write"
            );
        }
        assert!(
            !root
                .join(".prikk")
                .join("active")
                .join("default")
                .join("active.lock")
                .exists(),
            "the lock is released"
        );
        assert_eq!(
            std::fs::read(&older).expect("older file"),
            b"removed by a 0.48.0 repair",
            "nothing is deleted"
        );
        let listing = list(layout.repository_mutation_root()).expect("list");
        assert_eq!(
            listing.older_files,
            vec!["recovery/wal-default-at-3-0123456789abcdef.bytes".to_string()]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
