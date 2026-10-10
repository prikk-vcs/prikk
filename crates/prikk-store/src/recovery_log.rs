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

mod restore;
pub(crate) use restore::restore;

use prikk_error::{PrikkError, Result};

use crate::foundation::frame_resync::ScanBudget;
use crate::foundation::fsutil::{
    EntryKind, MutationRoot, append_file_required, create_new_file_required,
    ensure_directory_required, inspect_entry, list_directory, overwrite_in_place_required,
    read_file_if_exists, read_file_range_if_exists, stat_file_state_if_exists,
    truncate_existing_file_required,
};
use crate::foundation::generation::{self, resolve_live_slot_trusting_default_on_ambiguity};
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

/// What an entry records (RFC 168 A1, item 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Bytes a repair removed from the end of a file (the original RFC 168 entry).
    Cut,
    /// The previous bytes of a file a repair rewrote in place; `new_hash` identifies what the repair wrote.
    Replace,
}

/// One saved step of a repair (RFC 168 §3.1, A1). Entries of one repair share a `run` (A1 item 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) kind: Kind,
    /// The run this entry belongs to: one id per repair the user ran. Stamped by [`append`].
    pub(crate) run: [u8; 8],
    /// The source file, relative to the repository root, `/`-separated.
    pub(crate) source: String,
    /// `Cut`: the offset the source was cut at; the removed bytes start here. `Replace`: always 0.
    pub(crate) offset: u64,
    /// Which repair wrote the entry (for the listing and the plan).
    pub(crate) label: String,
    pub(crate) binary_version: String,
    /// `Cut`: SHA-256 of the source's bytes `[0, offset)` at repair time. `Replace`: SHA-256 of the previous bytes.
    pub(crate) prefix_hash: [u8; 32],
    pub(crate) meaning: Vec<Meaning>,
    /// `Cut`: the removed bytes. `Replace`: the previous bytes of the file.
    pub(crate) removed: Vec<u8>,
    /// `Replace`: SHA-256 of the bytes the repair wrote. Zero for `Cut`.
    pub(crate) new_hash: [u8; 32],
}

/// What a repair reports about the entry it wrote: enough to name it on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveryRef {
    /// The 16-hex-character run id: the id a user restores by. Empty for a plan-only preview, which writes nothing.
    pub id: String,
    /// The source the entry saved bytes from, relative to the repository root.
    pub source: String,
    /// The offset the source was cut at.
    pub offset: u64,
    /// How many bytes were saved.
    pub len: u64,
}

impl std::fmt::Display for RecoveryRef {
    /// How a repair names its run on screen: where it is, and the id to restore by.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.id.is_empty() {
            write!(f, "recovery/log (planned, nothing written)")
        } else {
            write!(f, "recovery/log, run {}", self.id)
        }
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
    /// Complete frames written by a newer prikk, which this version cannot read. Not damage (RFC 168 F12).
    pub(crate) newer_versions: usize,
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
    out.push(match entry.kind {
        Kind::Cut => 0,
        Kind::Replace => 1,
    });
    out.extend_from_slice(&entry.run);
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
    out.extend_from_slice(&entry.new_hash);
    out
}

/// The frame checksum: over the magic, version, length and body, hashed in place (RFC 168 F4: no copy of the body). The version
/// is the one the frame's own header carries, so a frame's version is trusted only after its checksum holds (RFC 168 item 14).
fn checksum_with_version(version: u16, body: &[u8]) -> [u8; 32] {
    let len = (body.len() as u64).to_be_bytes();
    crate::foundation::frame_resync::tallied_sha256_parts(&[
        MAGIC,
        &version.to_be_bytes(),
        &len,
        body,
    ])
}

/// The framed bytes of one entry.
fn frame(entry: &Entry) -> Vec<u8> {
    let body = encode_body(entry);
    let checksum = checksum_with_version(VERSION, &body);
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(body.len() as u64).to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    out
}

thread_local! {
    /// The run the repair on this thread is writing (A1 item 2): every entry it appends shares this id.
    static CURRENT_RUN: std::cell::Cell<Option<[u8; 8]>> = const { std::cell::Cell::new(None) };
}

/// Counts runs begun by this process, so two runs begun in the same instant still differ.
static RUN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A fresh run id: 8 bytes from a digest of the time, the process and a counter.
fn new_run_id() -> [u8; 8] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = RUN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let digest = sha(&[
        nanos.to_be_bytes().as_slice(),
        &std::process::id().to_be_bytes(),
        &count.to_be_bytes(),
    ]
    .concat());
    let mut id = [0_u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

/// Begin a run: every entry appended on this thread until the returned scope ends shares one run id (A1 item 2). A nested call
/// joins the run that is already open, so a repair that calls another repair writes one run.
pub(crate) fn begin_run() -> RunScope {
    let owned = CURRENT_RUN.with(|current| {
        if current.get().is_none() {
            current.set(Some(new_run_id()));
            true
        } else {
            false
        }
    });
    RunScope { owned }
}

/// Ends the run it began, when it was the outermost (see [`begin_run`]).
pub(crate) struct RunScope {
    owned: bool,
}

/// 0.51.0 step 1 Part C2 (C2, the review): the open run's own id, hex-encoded the same way
/// [`RecoveryEntryView::id`] is -- so a caller that just saved something under [`begin_run`] can
/// name the run id in its own report, rather than making the user find it in `--recovery-list`.
/// `None` when no run is open (a `--plan-only` preview never calls `begin_run` at all).
pub(crate) fn current_run_id_hex() -> Option<String> {
    CURRENT_RUN.with(|current| current.get().map(|run| prikk_hash::to_hex(&run)))
}

impl Drop for RunScope {
    fn drop(&mut self) {
        if self.owned {
            CURRENT_RUN.with(|current| current.set(None));
        }
    }
}

/// The run an entry appended now belongs to: the open run, or a run of its own for a save outside any repair.
fn run_for_append() -> [u8; 8] {
    CURRENT_RUN.with(|current| match current.get() {
        Some(run) => run,
        None => new_run_id(),
    })
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
    let kind = match r.take(1)?.first().copied()? {
        0 => Kind::Cut,
        1 => Kind::Replace,
        _ => return None,
    };
    let run: [u8; 8] = r.take(8)?.try_into().ok()?;
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
    let new_hash = r.hash()?;
    if r.at != body.len() {
        return None;
    }
    Some(Entry {
        kind,
        run,
        source,
        offset,
        label,
        binary_version,
        prefix_hash,
        meaning,
        removed,
        new_hash,
    })
}

/// Append one entry and return its reference. The first save creates `recovery/log` (`init` creates it too, so in a
/// new repository the first save is an append). The caller holds the repair's locks.
pub(crate) fn append(root: &MutationRoot, entry: &Entry) -> Result<RecoveryRef> {
    let mut stamped = entry.clone();
    stamped.run = run_for_append();
    let framed = frame(&stamped);
    ensure_directory_required(root, Path::new("recovery"))?;
    let path = Path::new(LOG_PATH);
    if read_file_range_if_exists(root, path, 0, 0)?.is_none() {
        // The log's first appearance is a new name: on Windows it is not durable (RFC 168 §3.1, residual (b)). A concurrent
        // first write may have created it already (RFC 168 F12): then this entry is appended like any other.
        match create_new_file_required(root, path, &framed) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                append_file_required(root, path, &framed)?;
            }
            Err(error) => {
                return Err(PrikkError::MalformedData(format!(
                    "creating recovery/log: {error}"
                )));
            }
        }
    } else {
        append_file_required(root, path, &framed).map_err(|error| {
            PrikkError::MalformedData(format!("appending recovery/log: {error}"))
        })?;
    }
    Ok(RecoveryRef {
        id: prikk_hash::to_hex(&stamped.run),
        source: entry.source.clone(),
        offset: entry.offset,
        len: entry.removed.len() as u64,
    })
}

/// What one position of the log holds.
enum Candidate {
    /// A sound entry, and the offset after it.
    Entry(Box<Listed>, usize),
    /// A complete frame written by a newer prikk (its version is above this one's), and the offset after it. Not damage.
    Newer(usize),
    /// Bytes that cannot be a frame, the start of one that runs past the end, or a short header: a prefix of one frame at
    /// the end of the log. Whether it is a torn tail depends on whether a sound frame follows it.
    Prefix,
    /// Bytes that are not a sound frame, and cannot be a prefix of one.
    Damage,
}

/// Read every sound entry, skipping damaged regions byte-wise to the next magic. The RFC 167 budget bounds the work: each
/// candidate is charged its header and its claimed body (clamped to the bytes that remain) **before** it is hashed (RFC 168 F4).
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
        budget.charge(claimed_span(&bytes, at));
        if budget.exceeded() {
            listing.unread_tail = true;
            break;
        }
        match classify_at(&bytes, at) {
            Candidate::Entry(listed, next) => {
                listing.entries.push(*listed);
                in_damage = false;
                at = next;
            }
            Candidate::Newer(next) => {
                listing.newer_versions += 1;
                in_damage = false;
                at = next;
            }
            Candidate::Prefix => {
                if sound_frame_after(&bytes, at + 1, &mut budget) == Some(false) {
                    // Nothing sound follows, and the question was answered: a torn tail, from an interrupted save (C2: nothing removed).
                    listing.torn_tail = true;
                    break;
                }
                if !in_damage {
                    listing.damaged_regions += 1;
                    in_damage = true;
                }
                at += 1;
            }
            Candidate::Damage => {
                if !in_damage {
                    listing.damaged_regions += 1;
                    in_damage = true;
                }
                at += 1;
            }
        }
    }
    Ok(listing)
}

/// The bytes a candidate at `at` may cost to examine: its header and its claimed body, clamped to what remains; one byte when
/// the position does not start with the magic.
fn claimed_span(bytes: &[u8], at: usize) -> u64 {
    let rest = bytes.get(at..).unwrap_or_default();
    if rest.len() < HEADER_LEN || rest.get(..8) != Some(MAGIC.as_slice()) {
        return 1;
    }
    let claimed = rest
        .get(10..18)
        .and_then(|field| <[u8; 8]>::try_from(field).ok())
        .map_or(0, u64::from_be_bytes);
    let body = usize::try_from(claimed)
        .unwrap_or(usize::MAX)
        .min(rest.len() - HEADER_LEN);
    (HEADER_LEN + body) as u64
}

/// Classify the position `at`. A complete candidate is checksummed in place, after the caller has charged it.
fn classify_at(bytes: &[u8], at: usize) -> Candidate {
    let rest = bytes.get(at..).unwrap_or_default();
    if rest.len() < HEADER_LEN {
        return if rest.get(..rest.len().min(8)) == MAGIC.get(..rest.len().min(8)) {
            Candidate::Prefix
        } else {
            Candidate::Damage
        };
    }
    if rest.get(..8) != Some(MAGIC.as_slice()) {
        return Candidate::Damage;
    }
    let version = rest
        .get(8..10)
        .and_then(|field| <[u8; 2]>::try_from(field).ok())
        .map_or(0, u16::from_be_bytes);
    let claimed = rest
        .get(10..18)
        .and_then(|field| <[u8; 8]>::try_from(field).ok())
        .map_or(0, u64::from_be_bytes);
    let Ok(body_len) = usize::try_from(claimed) else {
        return Candidate::Prefix;
    };
    if body_len > rest.len() - HEADER_LEN {
        return Candidate::Prefix;
    }
    let body_end = at + HEADER_LEN + body_len;
    let Some(body) = bytes.get(at + HEADER_LEN..body_end) else {
        return Candidate::Prefix;
    };
    let Some(stored) = rest
        .get(18..HEADER_LEN)
        .and_then(|field| <[u8; 32]>::try_from(field).ok())
    else {
        return Candidate::Damage;
    };
    // The checksum first: a version field that is not covered by a holding checksum is damage, not a newer frame (item 14).
    if checksum_with_version(version, body) != stored {
        return Candidate::Damage;
    }
    if version > VERSION {
        return Candidate::Newer(body_end);
    }
    if version != VERSION {
        return Candidate::Damage;
    }
    match decode_body(body) {
        Some(entry) => Candidate::Entry(
            Box::new(Listed {
                id: prikk_hash::to_hex(&entry.run),
                entry,
            }),
            body_end,
        ),
        None => Candidate::Damage,
    }
}

/// Whether a sound frame starts at or after `from`: `Some(true)`, `Some(false)`, or `None` when the budget ran out before the
/// question could be answered (an ambiguity, which resolves to damage: RFC 167 D2).
fn sound_frame_after(bytes: &[u8], from: usize, budget: &mut ScanBudget) -> Option<bool> {
    let mut cursor = from;
    while let Some(found) =
        crate::foundation::frame_resync::resync_to_next_magic(bytes, cursor, MAGIC)
    {
        budget.charge(claimed_span(bytes, found));
        if budget.exceeded() {
            return None;
        }
        if matches!(
            classify_at(bytes, found),
            Candidate::Entry(..) | Candidate::Newer(_)
        ) {
            return Some(true);
        }
        cursor = found + 1;
    }
    Some(false)
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
    // A1 (item 1): the commit witness and ref-name are rewritten by repairs, so their replace entries must restore too.
    let mut paths: Vec<PathBuf> = vec![
        layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME),
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
        layout.default_active_ref_name_path(),
    ];
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
    // A1 (item 1): the witness and ref-name are the state of the WAL they belong to, so the WAL is their meaning file.
    let wal_relative = relative(&layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME))?;
    if source
        == relative(
            &layout
                .active_session_dir(DEFAULT_ACTIVE_NAME)
                .join("witness"),
        )?
        || source == relative(&layout.default_active_ref_name_path())?
    {
        return Ok(vec![wal_relative]);
    }
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
    let ref_pointer_index_generation_log_path = layout.ref_pointer_index_generation_log_path();
    let ref_pointer_index_slot_b_path = layout.ref_pointer_index_slot_path(ContainerSlot::B);
    // Part E3 (the review): a restore is a deliberate writer, so it must not trust slot A in the
    // ambiguous state the way the best-effort default above does for every other caller -- a stale
    // meaning file would then compare unchanged and pass, though the live slot actually changed. The
    // existing cheap boolean check (no decode, no new dependency) is enough to refuse this one case.
    if generation::generation_log_lost(
        layout,
        &ref_pointer_index_generation_log_path,
        &ref_pointer_index_slot_b_path,
    )? {
        // Q2b (H1 ruling item 2): `compact --pointer-index` itself now refuses in this exact
        // state (deduced or ambiguous alike), so naming it as the way out would just hand the
        // caller a second refusal -- the rebuild is the one way out, for either sub-case.
        return Err(PrikkError::Integrity(
            "the ref pointer index's live slot is not recorded; run `prikk doctor \
             --rebuild-pointer-index --plan-only`, then `prikk doctor --rebuild-pointer-index` \
             first"
                .to_string(),
        ));
    }
    let pointer_slot = resolve_live_slot_trusting_default_on_ambiguity(
        layout,
        &ref_pointer_index_generation_log_path,
    )?;
    // Part F2 (the review): either slot, not only the one currently live. A rebuild's own undo
    // entries (`pointer_rebuild.rs`) name first the retiring slot, then the newly live one, as its
    // own run's two `Replace` entries -- liveness flips, by construction, between when each entry is
    // written and when a restore is later attempted, and flips again on a second restore of the same
    // run (A1 item 5). Conditioning this table's own answer on current liveness made a restore and a
    // second restore of the identical entry disagree with each other, never only with the writer.
    // Unconditional on either slot is also the more accurate rule: a slot's own claimed positions
    // need the ref log's agreement regardless of which slot happens to be live when asked.
    for slot in [ContainerSlot::A, ContainerSlot::B] {
        if source == relative(&layout.ref_pointer_index_slot_path(slot))? {
            return Ok(vec![relative(
                &layout.ref_log_container_slot_path(ContainerSlot::A),
            )?]);
        }
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
        kind: Kind::Cut,
        run: [0; 8],
        new_hash: [0; 32],
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
        kind: Kind::Cut,
        run: [0; 8],
        new_hash: [0; 32],
        source: source.to_string(),
        offset: 0,
        label: "object index lost ids".to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        prefix_hash: sha(&[]),
        meaning: Vec::new(),
        removed: contents.to_vec(),
    }
}

/// Save the previous bytes of a file a repair is about to rewrite in place (RFC 168 A1, item 1). Call it before the write: `previous`
/// are the file's bytes now, and `written` are the bytes the repair will write. The meaning files are taken now, too.
pub(crate) fn save_replace(
    layout: &RepositoryLayout,
    source: &str,
    previous: &[u8],
    written: &[u8],
    label: &str,
) -> Result<RecoveryRef> {
    let meaning = meaning_now(
        layout.repository_mutation_root(),
        &meaning_paths_for(layout, source)?,
    )?;
    let entry = Entry {
        kind: Kind::Replace,
        run: [0; 8],
        source: source.to_string(),
        offset: 0,
        label: label.to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        prefix_hash: sha(previous),
        meaning,
        removed: previous.to_vec(),
        new_hash: sha(written),
    };
    append(layout.repository_mutation_root(), &entry)
}

/// Save the commit witness's current bytes before a repair rewrites it (RFC 168 A1, item 1). `written` are the bytes the rewrite puts in
/// place; the witness's meaning is the WAL it covers, recorded now.
pub(crate) fn save_witness_replace(layout: &RepositoryLayout, written: &[u8]) -> Result<()> {
    let path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    save_current_replace(layout, &path, written, "witness")
}

/// Save ref-name's current bytes before a repair rewrites it (RFC 168 A1, item 1).
pub(crate) fn save_ref_name_replace(layout: &RepositoryLayout, written: &[u8]) -> Result<()> {
    let path = layout.default_active_ref_name_path();
    save_current_replace(layout, &path, written, "ref-name")
}

fn save_current_replace(
    layout: &RepositoryLayout,
    path: &Path,
    written: &[u8],
    label: &str,
) -> Result<()> {
    let relative = layout.repository_relative(path)?;
    let previous =
        read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default();
    let source = relative.to_string_lossy().replace('\\', "/");
    save_replace(layout, &source, &previous, written, label)?;
    Ok(())
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
pub(crate) fn verify_line(layout: &RepositoryLayout) -> Option<String> {
    // RFC 168 F1: a log that cannot be read is one line, and `verify`'s exit status does not change by it.
    let listing = match list(layout.repository_mutation_root()) {
        Ok(listing) => listing,
        Err(error) => return Some(format!("recovery log: cannot be read ({error})")),
    };
    if listing.damaged_regions == 0 && !listing.unread_tail {
        return None;
    }
    Some(format!(
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
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::foundation::layout::RepositoryLayout;
    use crate::test_gates::test_support::{signed_empty_block_envelope, signed_ref_state_envelope};
    use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore};

    fn publish(
        store: &RefStore,
        objects: &mut FileObjectStore,
        ref_name: &str,
    ) -> prikk_object::ObjectId {
        let target = objects
            .write_object(&signed_empty_block_envelope())
            .expect("write object");
        let ref_state = signed_ref_state_envelope(ref_name, None, target, 1);
        let ref_state_id = ref_state.object_id();
        store
            .publish(&RefPublication {
                ref_name: ref_name.to_string(),
                expected_previous_ref_state_id: None,
                ref_update: crate::test_gates::test_support::signed_ref_update_envelope(
                    ref_name,
                    None,
                    ref_state_id,
                    target,
                    1,
                ),
                ref_state,
            })
            .expect("publish");
        ref_state_id
    }

    /// Part E3 (the review): a restore must not trust slot A in the ambiguous state the way the
    /// best-effort default resolver does for every other caller -- it refuses instead.
    #[test]
    fn meaning_paths_for_refuses_when_the_pointer_index_live_slot_is_ambiguous() {
        let root = crate::test_gates::test_support::unique_temp_dir(
            "recovery-log-meaning-paths-ambiguous",
        );
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mut objects = FileObjectStore::new(layout.clone());
        let store = RefStore::new(layout.clone());
        publish(&store, &mut objects, "heads/main");

        crate::compact::compact_ref_pointer_index(&layout).expect("compact");
        std::fs::write(layout.ref_pointer_index_generation_log_path(), b"").expect("lose log");

        let ref_log_a = layout.repository_relative(
            &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
        );
        let ref_log_a = ref_log_a
            .expect("relative path")
            .to_string_lossy()
            .replace('\\', "/");
        let result = meaning_paths_for(&layout, &ref_log_a);
        let Err(error) = result else {
            panic!("expected a refusal, got {result:?}");
        };
        let error = error.to_string();
        assert!(error.contains("ref pointer index"), "{error}");
        assert!(error.contains("--rebuild-pointer-index"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    fn sample() -> Entry {
        Entry {
            kind: Kind::Cut,
            run: [0; 8],
            new_hash: [0; 32],
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
        let mut entry = sample();
        entry.run = [7; 8];
        let framed = frame(&entry);
        let id = prikk_hash::to_hex(&entry.run);
        assert_eq!(id.len(), ID_LEN);
        let Candidate::Entry(listed, next) = classify_at(&framed, 0) else {
            panic!("a sound frame classifies as an entry");
        };
        assert_eq!(listed.entry, entry);
        assert_eq!(listed.id, id);
        assert_eq!(next, framed.len());
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
            kind: Kind::Cut,
            run: [0; 8],
            new_hash: [0; 32],
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
        assert_eq!(verify_line(&layout), None);
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
                .is_some_and(|text| text.contains("a restore does not write FORMAT")),
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
#[non_exhaustive]
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
    /// 0.51.0 step 1 Part C2 (C4, the review): `Cut` removed trailing bytes at `offset`; `Replace`
    /// rewrote the whole file in place (`offset` is always 0 for this kind) -- printing both the
    /// same way as "cut at 0" read as a truncation even for a `--keep-slot` save, which replaces a
    /// whole file, not a tail.
    pub kind: RecoveryEntryKind,
}

/// A public mirror of [`Kind`] for [`RecoveryEntryView`] (which is `pub`, unlike `Kind` itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecoveryEntryKind {
    /// Trailing bytes a repair removed from the end of the file.
    Cut,
    /// The whole file's own previous bytes, before a repair rewrote it in place.
    Replace,
}

/// What `--recovery-list` prints: the sound entries, the damaged regions, and the older files listed by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveryListing {
    /// The sound entries, in log order.
    pub entries: Vec<RecoveryEntryView>,
    /// Damaged regions the resync skipped (never truncated, so they stay).
    pub damaged_regions: usize,
    /// Unreadable bytes at the end of the log: an interrupted save, harmless.
    pub torn_tail: bool,
    /// The RFC 167 budget stopped the scan before the end of the log.
    pub unread_tail: bool,
    /// Entries written by a newer prikk, which this version cannot read. Not damage.
    pub newer_versions: usize,
    /// The `recovery/*.bytes` files a repair wrote before this format: listed, never restored, never deleted (RFC 168 C4).
    pub older_files: Vec<String>,
}

/// One condition of a restore plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreConditionView {
    /// What the condition checks, in the words the plan prints.
    pub text: String,
    /// Whether it holds now.
    pub holds: bool,
}

/// What `--recovery-restore` prints first: each condition, what it would write, and whether it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestorePlanView {
    /// The run id the plan is for.
    pub id: String,
    /// The steps of the run, in the order they are undone (A1 item 3).
    pub steps: Vec<RestoreStepView>,
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

/// One entry `--recovery-clear` removes, listed before it does (RFC 168 F9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearedEntry {
    /// The entry's id.
    pub id: String,
    /// The source the entry saved bytes from.
    pub source: String,
    /// How many bytes it saved.
    pub len: u64,
}

/// One step of a run's restore, as `--recovery-restore` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreStepView {
    /// The file the step writes.
    pub source: String,
    /// `cut` (bytes appended at an offset) or `rewrite` (the previous bytes written in place).
    pub kind: String,
    /// Already holds what the step writes: skipped (A1 item 5).
    pub done: bool,
    /// The step's conditions, each with the fact found.
    pub conditions: Vec<RestoreConditionView>,
    /// How many bytes the step writes.
    pub bytes: usize,
}

/// What `--recovery-clear` removed, or would remove under `--plan-only`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveryClearView {
    /// Each sound entry, listed.
    pub entries: Vec<ClearedEntry>,
    /// How many bytes the log held.
    pub bytes: usize,
    /// Whether the log was emptied (false under `--plan-only`).
    pub cleared: bool,
}

/// The locks a repair takes, taken together by a restore and a clear (RFC 168 F5): the object-store lock, the ref-pointer-index,
/// ref-log, received-index and trust-policy container locks, and the active lock of every session on disk, the default included.
/// Active locks come first, then the containers, in the order the repairs take them.
fn repair_locks(
    layout: &RepositoryLayout,
) -> Result<(
    Vec<crate::lock::ActiveLock>,
    crate::lock::ContainerLockGuard,
)> {
    use crate::foundation::layout::LockableContainer;
    let mut sessions: std::collections::BTreeSet<std::ffi::OsString> =
        layout.active_session_names()?.into_iter().collect();
    sessions.insert(std::ffi::OsString::from(DEFAULT_ACTIVE_NAME));
    let mut actives = Vec::with_capacity(sessions.len());
    for name in &sessions {
        actives.push(crate::lock::ActiveLock::acquire(layout, name)?);
    }
    let containers = crate::lock::acquire_container_locks(
        layout,
        &[
            LockableContainer::ObjectStore,
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
            LockableContainer::TrustPolicy,
        ],
    )?;
    Ok((actives, containers))
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
                kind: match listed.entry.kind {
                    Kind::Cut => RecoveryEntryKind::Cut,
                    Kind::Replace => RecoveryEntryKind::Replace,
                },
            })
            .collect(),
        damaged_regions: listing.damaged_regions,
        torn_tail: listing.torn_tail,
        unread_tail: listing.unread_tail,
        newer_versions: listing.newer_versions,
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
        steps: plan
            .steps
            .iter()
            .map(|step| RestoreStepView {
                source: step.source.clone(),
                kind: match step.kind {
                    Kind::Cut => "cut".to_string(),
                    Kind::Replace => "rewrite".to_string(),
                },
                done: step.done,
                conditions: step
                    .conditions
                    .iter()
                    .map(|condition| RestoreConditionView {
                        text: condition.text.clone(),
                        holds: condition.holds,
                    })
                    .collect(),
                bytes: step.bytes,
            })
            .collect(),
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
        would_write: plan.would_write,
        refusal: plan.refusal,
        written: plan.written,
    })
}

/// `prikk doctor --recovery-clear [--plan-only]`: under the repair's locks, list the entries and, unless `plan_only`, empty the log
/// in place (the name is kept).
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
        entries: listing
            .entries
            .iter()
            .map(|listed| ClearedEntry {
                id: listed.id.clone(),
                source: listed.entry.source.clone(),
                len: listed.entry.removed.len() as u64,
            })
            .collect(),
        bytes,
        cleared,
    })
}

/// The one line `verify` prints about the log, or `None` when the log is sound and fully read. Never changes `verify`'s exit
/// status (RFC 168 §3.1).
pub fn recovery_verify_line(layout: &RepositoryLayout) -> Option<String> {
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
            let _lock = ActiveLock::acquire_for_write(&layout, DEFAULT_ACTIVE_NAME)
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod review_fixes {
    use super::*;
    use crate::foundation::frame_resync::hash_tally;
    use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
    use crate::test_gates::test_support::unique_temp_dir;

    fn entry(source: &str, removed: &[u8]) -> Entry {
        Entry {
            kind: Kind::Cut,
            run: [0; 8],
            new_hash: [0; 32],
            source: source.to_string(),
            offset: 4,
            label: "review".to_string(),
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            prefix_hash: sha(b"abcd"),
            meaning: Vec::new(),
            removed: removed.to_vec(),
        }
    }

    fn log_path(root: &std::path::Path) -> std::path::PathBuf {
        root.join(".prikk").join(LOG_PATH)
    }

    /// RFC 168 F3: a short header is a torn tail.
    #[test]
    fn a_short_header_at_the_end_is_a_torn_tail() {
        let root = unique_temp_dir("recovery-log-f3-short");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        append(layout.repository_mutation_root(), &entry("FORMAT", b"x")).expect("append");
        let mut bytes = std::fs::read(log_path(&root)).expect("read");
        bytes.extend_from_slice(&MAGIC[..5]);
        std::fs::write(log_path(&root), &bytes).expect("write");
        let listing = list(layout.repository_mutation_root()).expect("list");
        assert!(listing.torn_tail);
        assert_eq!(listing.damaged_regions, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F3: a sound magic whose body runs past the end of the file is a torn tail, when nothing sound follows it.
    #[test]
    fn a_body_that_runs_past_the_end_is_a_torn_tail() {
        let root = unique_temp_dir("recovery-log-f3-past-end");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        append(layout.repository_mutation_root(), &entry("FORMAT", b"x")).expect("append");
        let mut bytes = std::fs::read(log_path(&root)).expect("read");
        let mut header = Vec::new();
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_be_bytes());
        header.extend_from_slice(&1000_u64.to_be_bytes());
        header.extend_from_slice(&[0_u8; 32]);
        header.extend_from_slice(b"short body");
        bytes.extend_from_slice(&header);
        std::fs::write(log_path(&root), &bytes).expect("write");
        let listing = list(layout.repository_mutation_root()).expect("list");
        assert_eq!(listing.entries.len(), 1);
        assert!(listing.torn_tail);
        assert_eq!(listing.damaged_regions, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F3: a complete last frame that fails its checksum is damage, listed and printed by `verify`. One byte flipped in
    /// the only entry is the reproduced case.
    #[test]
    fn a_complete_frame_at_the_end_that_fails_its_checksum_is_damage() {
        let root = unique_temp_dir("recovery-log-f3-flipped");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        append(
            layout.repository_mutation_root(),
            &entry("FORMAT", b"only entry"),
        )
        .expect("append");
        let mut bytes = std::fs::read(log_path(&root)).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(log_path(&root), &bytes).expect("write");
        let listing = list(layout.repository_mutation_root()).expect("list");
        assert!(listing.entries.is_empty());
        assert_eq!(listing.damaged_regions, 1, "the flipped entry is damage");
        assert!(!listing.torn_tail, "a complete frame is never a torn tail");
        assert!(verify_line(&layout).is_some_and(|line| line.contains("1 damaged region")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F12: a frame written by a newer prikk lists as such, and is not damage.
    #[test]
    fn a_newer_version_frame_is_not_damage() {
        let root = unique_temp_dir("recovery-log-f12-newer");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        append(layout.repository_mutation_root(), &entry("FORMAT", b"old")).expect("append");
        let body = encode_body(&entry("FORMAT", b"newer"));
        let len = (body.len() as u64).to_be_bytes();
        let version: u16 = VERSION + 1;
        let checksum = crate::foundation::frame_resync::tallied_sha256_parts(&[
            MAGIC,
            &version.to_be_bytes(),
            &len,
            &body,
        ]);
        let mut framed = Vec::new();
        framed.extend_from_slice(MAGIC);
        framed.extend_from_slice(&version.to_be_bytes());
        framed.extend_from_slice(&len);
        framed.extend_from_slice(&checksum);
        framed.extend_from_slice(&body);
        let mut bytes = std::fs::read(log_path(&root)).expect("read");
        bytes.extend_from_slice(&framed);
        std::fs::write(log_path(&root), &bytes).expect("write");
        let listing = list(layout.repository_mutation_root()).expect("list");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.newer_versions, 1);
        assert_eq!(listing.damaged_regions, 0);
        assert_eq!(
            verify_line(&layout),
            None,
            "a newer entry prints no damage line"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F4: a damaged entry whose payload holds many in-range `PRECLOG1` headers keeps the bytes hashed within the budget's
    /// 8× of the input. Without the charge before hashing, every candidate hashes its whole claimed body: quadratic in the input.
    #[test]
    fn a_payload_full_of_headers_keeps_the_bytes_hashed_within_eight_times_the_input() {
        let root = unique_temp_dir("recovery-log-f4-budget");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mut bytes = Vec::new();
        while bytes.len() < 256 * 1024 {
            let remaining = 256 * 1024 - bytes.len();
            let claimed = remaining.saturating_sub(HEADER_LEN) as u64;
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&VERSION.to_be_bytes());
            bytes.extend_from_slice(&claimed.to_be_bytes());
            bytes.extend_from_slice(&[0_u8; 32]);
            bytes.extend_from_slice(&[0xAB_u8; 64]);
        }
        let input = bytes.len();
        std::fs::write(log_path(&root), &bytes).expect("write");
        hash_tally::reset();
        let listing = list(layout.repository_mutation_root()).expect("list");
        let hashed = hash_tally::bytes_hashed();
        assert!(
            listing.entries.is_empty(),
            "nothing in the payload is a sound entry"
        );
        assert!(
            hashed <= 8 * input as u64,
            "hashed {hashed} bytes of a {input}-byte log: more than the budget's 8x"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F8: an entry whose meaning list differs from the table refuses, and names why.
    #[test]
    fn a_restore_refuses_an_entry_whose_meaning_list_differs_from_the_table() {
        let root = unique_temp_dir("recovery-log-f8-meaning");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let wal = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
        let source = layout
            .repository_relative(&wal)
            .expect("relative")
            .to_string_lossy()
            .replace('\\', "/");
        let mut forged = entry(&source, b"removed");
        forged.meaning = Vec::new();
        let reference = append(layout.repository_mutation_root(), &forged).expect("append");
        let plan = restore(&layout, &reference.id, true).expect("plan");
        assert!(!plan.written);
        assert!(
            plan.refusal
                .as_deref()
                .is_some_and(|text| text.contains("does not match the table")),
            "{plan:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F9: ids compare ignoring case.
    #[test]
    fn an_uppercase_id_names_the_same_entry() {
        let root = unique_temp_dir("recovery-log-f9-case");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let reference =
            append(layout.repository_mutation_root(), &entry("FORMAT", b"x")).expect("append");
        let plan = restore(&layout, &reference.id.to_uppercase(), true).expect("plan");
        // The id resolves to the entry: the refusal is the allowlist's (FORMAT is not on the repair list), not "no entry".
        assert!(
            plan.refusal
                .as_deref()
                .is_some_and(|text| text.contains("a restore does not write FORMAT")),
            "{plan:?}"
        );
        assert_eq!(plan.id, reference.id);
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F6: plan-only paths create nothing: the log and the witness stay absent.
    #[test]
    fn plan_only_restore_and_clear_create_nothing() {
        let root = unique_temp_dir("recovery-log-f6-plan");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let witness = layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness");
        std::fs::remove_file(root.join(".prikk").join(LOG_PATH)).expect("remove the log");
        std::fs::remove_file(&witness).expect("remove the witness");
        let _ = crate::recovery_restore(&layout, &"0".repeat(16), true);
        let _ = crate::recovery_clear(&layout, true);
        let _ = crate::plan_discard_damaged_commits(&layout);
        let _ = crate::plan_restore_queue_target(&layout, "heads/main", false);
        assert!(
            !root.join(".prikk").join(LOG_PATH).exists(),
            "plan-only restore and clear create no log"
        );
        assert!(
            !witness.exists(),
            "plan-only restore and clear create no witness"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 F5: restore and clear take the object-store lock too: while an index rebuild holds it, neither runs.
    #[test]
    fn restore_and_clear_wait_for_the_object_store_lock() {
        use crate::foundation::layout::LockableContainer;
        let root = unique_temp_dir("recovery-log-f5-locks");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let held = crate::lock::acquire_container_locks(&layout, &[LockableContainer::ObjectStore])
            .expect("hold it");
        assert!(
            crate::recovery_restore(&layout, &"0".repeat(16), true).is_err(),
            "restore waits for the lock"
        );
        assert!(
            crate::recovery_clear(&layout, true).is_err(),
            "clear waits for the lock"
        );
        drop(held);
        assert!(
            crate::recovery_clear(&layout, true).is_ok(),
            "and runs once it is free"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// RFC 168 item 14: a flipped byte in a sound entry's version field is damage, never a newer frame, and the entry after it stays
    /// listed. Control: taking the version from the constant instead of the header turns this red.
    #[test]
    fn a_flipped_version_byte_is_damage_and_the_entry_after_it_stays_listed() {
        for offset in [8_usize, 9] {
            let root = unique_temp_dir("recovery-log-item14-version");
            let layout = RepositoryLayout::init(root.clone()).expect("init");
            let mutation = layout.repository_mutation_root();
            append(mutation, &entry("FORMAT", b"first")).expect("first");
            let second = append(mutation, &entry("FORMAT", b"second")).expect("second");
            let path = root.join(".prikk").join(LOG_PATH);
            let mut bytes = std::fs::read(&path).expect("read");
            bytes[offset] ^= 0x01;
            std::fs::write(&path, &bytes).expect("write");
            let listing = list(mutation).expect("list");
            assert_eq!(
                listing.newer_versions, 0,
                "byte {offset}: a flipped version is not a newer frame"
            );
            assert_eq!(
                listing.damaged_regions, 1,
                "byte {offset}: the flipped frame is damage"
            );
            let ids: Vec<&str> = listing
                .entries
                .iter()
                .map(|listed| listed.id.as_str())
                .collect();
            assert_eq!(
                ids,
                vec![second.id.as_str()],
                "byte {offset}: the entry after it stays listed"
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
