//! AUTHOR key-material container (DC-53 Stage 1, restaged per
//! `.git-exclude/reviewed/DC-53-stage-1-report-ruling-v1.md` §2-§3).
//!
//! **Material only -- no admission judgement, no conflict rejection.** This is deliberately the
//! narrower half of what `trust_index.rs` provides for MAINTAINER keys: that module also has a
//! *policy* container (which key ids are currently adopted), layered over its key-material
//! container. AUTHOR trust has no policy layer yet -- Stage 2 owns TOFU conflict semantics -- so
//! this module mirrors only `trust_index.rs`'s key-material half, not its policy half.
//!
//! **Why this container exists at all, and why it cannot be populated the way MAINTAINER's is.**
//! Ed25519 signatures are not public-key-recoverable: verifying one requires the verifier to already
//! hold the claimed public key, and nothing else in a persisted `Patch` carries it (`Signature`
//! carries a `key_id` *label*, `signature.rs:83`; `PatchPayload` carries no key field at all). MAINTAINER
//! key material is supplied by an explicit adoption act (`trust::add_trusted_maintainer`) that a
//! repository operator performs once, deliberately. **There is no equivalent act for authors** --
//! observing a Patch's signature teaches nothing about the key that produced it, so pinning on first
//! *sight* (as this module's own first draft assumed) has no source to pin from. **AUTHOR key
//! material is therefore recorded at authoring time, by the signer itself**, the only party that
//! ever holds the public key -- see `record_author_key_material`'s callers in `worktree_patch/-
//! node_authoring.rs` and `rollback_draft.rs`.
//!
//! **Consequence, stated rather than discovered later:** every Patch authored before this
//! increment -- and every Patch authored by a signer this repository's own `RepositoryLayout` never
//! observed -- has no recorded key material and can never be verified by any future work. That is
//! not leniency; it is unverifiable in principle, and `verify` must report it as a distinct outcome
//! (DC-53's restaged D3, second row), not silently as sound.
//!
//! **One `key_id` binds to one public key (DC-53 Stage 2, D8).** Stage 1 shipped without conflict
//! rejection -- it appended a second, different key for an existing `key_id` rather than refusing it,
//! deferring TOFU conflict semantics to Stage 2. Stage 2 closes that: [`record_author_key_material`]
//! now refuses a conflicting key outright (`Err`), and `verify` (`verify_author_signature`) treats a
//! `key_id` that already carries more than one distinct recorded key as an authorship-integrity
//! failure, the same as a signature that does not verify -- D3's fourth row.
//!
//! **This state has no way out.** There is no prune, remove, or rewrite for this container (unlike
//! `trust_index.rs`'s maintainer material, it is not among `compact.rs`'s targets), and no `doctor`
//! repair. Once two distinct keys are recorded under one `key_id` -- which Stage 2's own
//! `record_author_key_material` now refuses to create, but a Stage-1-era repository may already carry
//! from before this rule existed -- `verify` fails for every Patch signed under that `key_id`,
//! permanently. This is fail-closed by deliberate choice, not an oversight: D8's ruling is that a
//! `key_id` whose recorded material disagrees with itself is not a state `verify` may paper over.
//! `record_author_key_material`'s rejection is what keeps a healthy repository from ever reaching it
//! going forward; nothing in this module can recover one that already has.
//!
//! **Why this reads as "no key rotation," not a defect (D5).** There is no rotation mechanism --
//! signing a fresh keypair under the same `key_id` you have always used is, from this container's
//! point of view, indistinguishable from an attacker who compromised the `key_id` and is asserting a
//! different key for it. Stage 2 refuses both identically. See
//! `docs/src/reference/trust-threat-model.md`'s rotation caveat.
//!
//! `lookup_author_key_entries` still returns *every* entry ever recorded for a `key_id`, not just the
//! most recent -- that return shape did not change; what changed is that Stage 2 forbids more than one
//! *distinct* public key from ever accumulating there through this module's own write path.

use prikk_crypto::{ED25519_KEY_LEN, verify_ed25519};
use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::{ObjectEnvelope, Signature, SignatureAlgorithm, SignerRole};

use std::path::Path;

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::push_string_u16;
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{
    append_file_required, create_new_file_required, len_to_u64, read_file_if_exists,
};
use crate::foundation::layout::RepositoryLayout;
use crate::lock::ActiveLock;

const AUTHOR_KEY_MAGIC: &[u8; 8] = b"PAUTKEY1";
const AUTHOR_KEY_VERSION: u16 = 1;
const AUTHOR_KEY_HEADER_LEN: usize = 8 + 2 + 8 + 32;

/// One recorded AUTHOR key's material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorKeyEntry {
    pub(crate) key_id: String,
    pub(crate) public_key: [u8; ED25519_KEY_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthorKeyRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorKeyRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: AuthorKeyRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorKeyReplay {
    pub(crate) entries: Vec<AuthorKeyEntry>,
    pub(crate) trailing_partial_bytes: usize,
    pub(crate) record_outcomes: Vec<AuthorKeyRecordOutcome>,
}

impl AuthorKeyReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, AuthorKeyRecordStatus::Failed { .. }))
    }
}

fn encode_author_key_body(entry: &AuthorKeyEntry) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    push_string_u16(&mut body, &entry.key_id)?;
    body.extend_from_slice(&entry.public_key);
    Ok(body)
}

fn decode_author_key_body(body: &[u8]) -> Result<AuthorKeyEntry> {
    let mut cursor = ByteCursor::new(body);
    let key_id = cursor.read_string_u16()?;
    let public_key = cursor.read_array::<ED25519_KEY_LEN>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in author key entry body".to_string(),
        ));
    }
    Ok(AuthorKeyEntry { key_id, public_key })
}

fn encode_author_key_record(entry: &AuthorKeyEntry) -> Result<Vec<u8>> {
    let body = encode_author_key_body(entry)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = author_key_checksum(body_len, &body);
    let mut out = Vec::with_capacity(AUTHOR_KEY_HEADER_LEN + body.len());
    out.extend_from_slice(AUTHOR_KEY_MAGIC);
    crate::foundation::file_codec::push_u16(&mut out, AUTHOR_KEY_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn author_key_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(AUTHOR_KEY_MAGIC);
    preimage.extend_from_slice(&AUTHOR_KEY_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

enum AuthorKeyFrameAttempt {
    Record {
        entry: AuthorKeyEntry,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
    },
}

fn parse_author_key_frame_at(bytes: &[u8], offset: usize) -> AuthorKeyFrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < AUTHOR_KEY_HEADER_LEN {
        return AuthorKeyFrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + AUTHOR_KEY_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return AuthorKeyFrameAttempt::TrailingPartial { remaining };
    };
    let mut cursor = ByteCursor::new(header);
    let (magic, version, body_len, checksum) = match (|| -> Result<_> {
        let magic = cursor.read_array::<8>()?;
        let version = cursor.read_u16()?;
        let body_len = cursor.read_u64()?;
        let checksum = cursor.read_array::<32>()?;
        Ok((magic, version, body_len, checksum))
    })() {
        Ok(values) => values,
        Err(err) => {
            return AuthorKeyFrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    if &magic != AUTHOR_KEY_MAGIC {
        return AuthorKeyFrameAttempt::Invalid {
            message: "invalid author key record magic".to_string(),
        };
    }
    if version != AUTHOR_KEY_VERSION {
        return AuthorKeyFrameAttempt::Invalid {
            message: format!("unsupported author key record version {version}"),
        };
    }
    let Ok(body_len_usize) = usize::try_from(body_len) else {
        return AuthorKeyFrameAttempt::Invalid {
            message: "author key body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len_usize) else {
        return AuthorKeyFrameAttempt::Invalid {
            message: "author key body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return AuthorKeyFrameAttempt::TrailingPartial { remaining };
    };
    let expected = author_key_checksum(body_len, body);
    if expected != checksum {
        return AuthorKeyFrameAttempt::Invalid {
            message: format!("author key checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_author_key_body(body) {
        Ok(entry) => AuthorKeyFrameAttempt::Record {
            entry,
            next_offset: body_end,
        },
        Err(err) => AuthorKeyFrameAttempt::Invalid {
            message: err.to_string(),
        },
    }
}

/// Isolate-and-continue reading, matching every other container's decode loop in this codebase
/// (`trust_index.rs::decode_trust_key_records` is the closest precedent).
fn decode_author_key_records(bytes: &[u8]) -> Result<AuthorKeyReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_author_key_frame_at(bytes, offset) {
            AuthorKeyFrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(AuthorKeyRecordOutcome {
                    offset,
                    status: AuthorKeyRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = next_offset;
            }
            AuthorKeyFrameAttempt::TrailingPartial { remaining } => {
                return Ok(AuthorKeyReplay {
                    entries,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            AuthorKeyFrameAttempt::Invalid { message } => {
                record_outcomes.push(AuthorKeyRecordOutcome {
                    offset,
                    status: AuthorKeyRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, AUTHOR_KEY_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(AuthorKeyReplay {
                            entries,
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                }
            }
        }
    }
}

fn replay_author_keys(layout: &RepositoryLayout) -> Result<AuthorKeyReplay> {
    let relative = layout.repository_relative(&layout.author_key_container_path())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        // A repository initialized before this container existed has no such file; that reads
        // identically to an empty one (no key material recorded for anything) -- exactly the
        // "unverifiable, not failed" outcome D3's second row describes, not a structural defect.
        return Ok(AuthorKeyReplay {
            entries: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_author_key_records(&bytes)
}

/// Every entry ever recorded for `key_id`, oldest first -- deliberately not just the most recent
/// one; see the module doc's "no conflict rejection" note. Refuses if the container has any damaged
/// entry, matching `trust_index::lookup_trust_key_entry`'s fail-closed reasoning.
pub(crate) fn lookup_author_key_entries(
    layout: &RepositoryLayout,
    key_id: &str,
) -> Result<Vec<AuthorKeyEntry>> {
    let replay = replay_author_keys(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "author key container has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .filter(|entry| entry.key_id == key_id)
        .collect())
}

/// Whether `key_id`/`public_key` is already on file, or would be a fresh append -- the read half of
/// the conflict rule, returned so a caller can tell the two apart without re-deriving the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorKeyCheck {
    AlreadyRecorded,
    New,
}

/// Checks `key_id`/`public_key` against this repository's existing author-key material **without
/// recording anything**. This is the single definition of what counts as a conflict (DC-53 Stage 2,
/// D8: one `key_id` binds to one public key) -- `record_author_key_material` below is a thin
/// check-then-append wrapper around it, and `bundle.rs`'s `import_bundle` calls it directly to
/// validate an entire transported key set before recording any entry of it, so a caller that must
/// validate many entries up front does not hand-write a second notion of conflict (DC-53 Stage 2
/// follow-up, `multi-key-import-partial-write-v1.md`).
/// What this repository has recorded for one author `key_id`, against one public key.
///
/// RFC 150 §2: the three-valued answer `prikk key status` reports, and the **same computation**
/// `check_author_key_conflict` below turns into its refusal — so a front-end asking "will this key
/// be accepted here?" and `commit` answering it cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorKeyBinding {
    /// No entry for this id: any seed is accepted and becomes the recording.
    Unrecorded,
    /// An entry for this id exists and matches this public key.
    Matches,
    /// An entry exists under this id with different key material. `commit` refuses.
    Mismatch,
}

/// Ask what [`AuthorKeyBinding`] holds for `key_id` and `public_key`, reading and never refusing.
pub fn author_key_binding(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key: [u8; ED25519_KEY_LEN],
) -> Result<AuthorKeyBinding> {
    let existing = lookup_author_key_entries(layout, key_id)?;
    if existing.iter().any(|entry| entry.public_key == public_key) {
        return Ok(AuthorKeyBinding::Matches);
    }
    if existing.is_empty() {
        return Ok(AuthorKeyBinding::Unrecorded);
    }
    Ok(AuthorKeyBinding::Mismatch)
}

pub(crate) fn check_author_key_conflict(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key: [u8; ED25519_KEY_LEN],
) -> Result<AuthorKeyCheck> {
    // RFC 150 §2: the three-valued answer is computed once, in `author_key_binding` above, and this
    // function is the half that turns `Mismatch` into the refusal. `key status` reads the same
    // query, so the two cannot drift -- `author_binding_and_commit_agree` perturbs it to prove so.
    let existing = lookup_author_key_entries(layout, key_id)?;
    match author_key_binding(layout, key_id, public_key)? {
        AuthorKeyBinding::Matches => return Ok(AuthorKeyCheck::AlreadyRecorded),
        AuthorKeyBinding::Unrecorded => return Ok(AuthorKeyCheck::New),
        AuthorKeyBinding::Mismatch => {}
    }
    if let Some(conflicting) = existing.first() {
        return Err(PrikkError::Integrity(format!(
            "author key_id {key_id} already has a different recorded public key ({}); one key_id \
             binds to one public key -- this looks like a key-rotation attempt, which is not \
             supported and is indistinguishable from impersonation",
            prikk_hash::to_hex(&conflicting.public_key)
        )));
    }
    Ok(AuthorKeyCheck::New)
}

/// Record `key_id`'s public key at authoring time. Idempotent if this exact `(key_id, public_key)`
/// pair was already recorded (an author signing many patches over time must not grow the container
/// once per patch). **Refuses (DC-53 Stage 2, D8) if a *different* public key is already on file for
/// `key_id`** -- one `key_id` binds to one public key; see the module doc for why this state, once
/// created, has no way out, and why this is what a key-rotation attempt looks like from the inside.
///
/// **Takes `&ActiveLock` structurally, not as a documented precondition** (DC-53 Stage 2 Step 2
/// review, superseding Step 1's own "callers must hold `ActiveLock`" comment): the read-then-append
/// below is a check-then-act, and the resulting conflict state is unrecoverable, so a barrier test
/// proving today's three callers happen to hold the lock would prove nothing about a fourth caller
/// added tomorrow without one. Requiring the guard in the signature makes the hazard impossible to
/// construct rather than something to police by review -- the lock parameter is otherwise unused by
/// this function's own body; its only job is to exist. `worktree_patch/node_authoring.rs`,
/// `rollback_draft.rs`, and `bundle.rs`'s `import_bundle` all pass their own held `ActiveLock`.
pub fn record_author_key_material(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key: [u8; ED25519_KEY_LEN],
    _active_lock: &ActiveLock,
) -> Result<()> {
    if check_author_key_conflict(layout, key_id, public_key)? == AuthorKeyCheck::AlreadyRecorded {
        return Ok(());
    }
    let record = encode_author_key_record(&AuthorKeyEntry {
        key_id: key_id.to_string(),
        public_key,
    })?;
    let relative = layout.repository_relative(&layout.author_key_container_path())?;
    ensure_author_key_container_exists(layout, &relative)?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

/// A repository initialized before this container existed has no such file. `append_file_required`
/// -- unlike the read path -- requires the target to already exist, so a repository this old fails to
/// author until its container is created. Create it lazily, on first write, so `layout.rs`'s "no
/// format bump, no migration step" claim holds for writes as well as reads. Crash-safe by inspection:
/// a crash between the create and the append below leaves an empty container, which reads identically
/// to a repository that has never authored anything yet. Tolerates a losing race against a concurrent
/// creator (`AlreadyExists`) -- the desired end state, the container existing, holds either way; the
/// callers that reach this do not hold a lock scoped to this specific container.
fn ensure_author_key_container_exists(layout: &RepositoryLayout, relative: &Path) -> Result<()> {
    if read_file_if_exists(layout.repository_mutation_root(), relative)?.is_some() {
        return Ok(());
    }
    match create_new_file_required(layout.repository_mutation_root(), relative, &[]) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Plant a second, distinct public key for an already-recorded `key_id`, bypassing
/// `record_author_key_material`'s own rejection -- for tests only, to construct the state DC-53
/// Stage 2's own rule is meant to make unreachable through normal operation (a legacy Stage-1-era
/// repository, or a race this repository's own write path no longer permits). Production code has no
/// equivalent: creating this state is exactly what Stage 2 exists to prevent.
// RFC 149 §6c: a test helper by name and by purpose; it travels with the tests that use it.
#[cfg(any(test, feature = "test-support"))]
pub fn force_conflicting_author_key_entry_for_test(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key: [u8; ED25519_KEY_LEN],
) -> Result<()> {
    let record = encode_author_key_record(&AuthorKeyEntry {
        key_id: key_id.to_string(),
        public_key,
    })?;
    let relative = layout.repository_relative(&layout.author_key_container_path())?;
    ensure_author_key_container_exists(layout, &relative)?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

/// Check `envelope`'s AUTHOR signature (DC-53 Stage 1, D3's first three rows; Stage 2, D3's fourth).
/// Returns `Ok(None)` if the envelope carries no AUTHOR-role signature at all -- out of this
/// increment's scope; a Patch with no AUTHOR signature is a pre-existing structural gap this
/// increment does not newly police, not something Stage 1 introduces a defect for. Returns
/// `Ok(Some((key_id, sound)))` otherwise, `sound` distinguishing D3's first row (verifies against
/// recorded material, `true`) from its second (no material recorded for this `key_id`, `false` --
/// **not** a failure). Returns `Err` for D3's third row (material *is* recorded and the signature
/// does not verify against any of it) and for its fourth (DC-53 Stage 2, D8: more than one *distinct*
/// public key is recorded for this `key_id` -- checked, and failed, before any signature-verification
/// attempt, since D8's rule is an invariant on the recorded material itself, not a claim about
/// whether this particular signature happens to verify against one of the conflicting keys) --
/// propagated so the caller's item-containment reports this Patch as failed, matching
/// forgery/corruption rather than the softer "issue" shape MAINTAINER admission failures use.
pub(crate) fn verify_author_signature(
    layout: &RepositoryLayout,
    envelope: &ObjectEnvelope,
) -> Result<Option<(String, bool)>> {
    let Some(signature) = envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Author)
    else {
        return Ok(None);
    };
    let entries = lookup_author_key_entries(layout, &signature.key_id)?;
    verify_author_signature_against_material(envelope, &entries)
}

/// The verification core, extracted so there is exactly one definition of *how* an AUTHOR signature
/// is checked, with two key sources (RFC 115 Stage 3 handoff §4.2 item 7): `verify_author_signature`
/// above passes this repository's own recorded entries for the signature's `key_id`;
/// the exchange accept path passes the union of the artifact's own transported material and this
/// repository's already-recorded material for that `key_id` -- neither source alone is complete at
/// the accept path's own verification phase, since recording the artifact's material happens later,
/// under lock, and only if the whole exchange succeeds (§4.1: nothing may be recorded from an
/// exchange that fails). Same shape `check_author_key_conflict` was extracted into, for the same
/// reason: one policy, parameterized by material source, never a second parallel copy of it.
///
/// `candidate_entries` must already be scoped to the signature's own `key_id` -- this function does
/// not filter by `key_id` itself, matching `lookup_author_key_entries`'s own output shape, which is
/// what `verify_author_signature` passes directly.
pub(crate) fn verify_author_signature_against_material(
    envelope: &ObjectEnvelope,
    candidate_entries: &[AuthorKeyEntry],
) -> Result<Option<(String, bool)>> {
    let Some(signature) = envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Author)
    else {
        return Ok(None);
    };
    if signature.algorithm != SignatureAlgorithm::Ed25519 {
        return Err(PrikkError::InvalidSignature(
            "AUTHOR signature is not Ed25519".to_string(),
        ));
    }
    if candidate_entries.is_empty() {
        return Ok(Some((signature.key_id.clone(), false)));
    }
    // DC-53 Stage 2, D8, D3's fourth row: one key_id binds to one public key. A key_id whose
    // recorded material already disagrees with itself is structurally unsound regardless of whether
    // this particular signature would verify against one of the conflicting entries -- checked, and
    // failed, before attempting verification at all (ratified explicitly in the Stage 2 Step 1
    // review: this is the correct order, not inferred).
    let first_public_key = candidate_entries.first().map(|entry| entry.public_key);
    if candidate_entries
        .iter()
        .any(|entry| Some(entry.public_key) != first_public_key)
    {
        return Err(PrikkError::Integrity(format!(
            "author key_id {} has more than one distinct recorded public key -- authorship \
             integrity for this key_id cannot be established",
            signature.key_id
        )));
    }
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        envelope.object_type,
        envelope.object_id(),
        SignerRole::Author,
        &signature.key_id,
    )?;
    let verifies = candidate_entries.iter().any(|entry| {
        verify_ed25519(&entry.public_key, &preimage, &signature.signature_bytes).is_ok()
    });
    if !verifies {
        return Err(PrikkError::InvalidSignature(format!(
            "{} {} AUTHOR signature does not verify against recorded key material for {}",
            envelope.object_type,
            envelope.object_id(),
            signature.key_id
        )));
    }
    Ok(Some((signature.key_id.clone(), true)))
}

#[cfg(test)]
mod tests;
