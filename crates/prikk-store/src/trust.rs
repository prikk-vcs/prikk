//! Repository-local publication trust store: the set of MAINTAINER keys this repository accepts as
//! having sealed a `Block`/`RefState` (DC-78 §D2). `required = 1` keeps its DC-11 meaning regardless
//! of how many keys are adopted — a block needs *one* trusted signature, never a threshold of
//! several (`rfcs/done/DC-78-HISTORY-EXCHANGE.md` §D2, confirmed against every existing
//! assumption at §D7.2). `required` is never stored anywhere -- it is a constant, and storing it
//! would let it drift into a count (design-v1.md §14 constraint 4).
//!
//! RFC 102 Stage 5, design-v1.md §14/§14.9: key material and policy are each on their own container
//! (`trust_index.rs`), replacing `trust/keys/maintainer/*.pub` and `trust/policy.toml` outright. The
//! policy is snapshot-based -- each write is the *complete* adopted-key-id list, not an incremental
//! append -- which is what makes `remove_trusted_maintainer` representable natively: append a snapshot
//! with the id absent, the same operation `add_trusted_maintainer` already performs, just shorter.

use prikk_crypto::{ED25519_KEY_LEN, verify_ed25519};
use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, Signature, SignatureAlgorithm, SignerRole, ascii_fold};

use crate::foundation::layout::{
    DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout,
    validate_maintainer_key_id_storage_safety,
};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::maintainer_signing::MaintainerSigner;
use crate::trust_index::{
    TrustKeyEntry, append_trust_key_entry, append_trust_policy_snapshot, lookup_trust_key_entry,
    lookup_trust_key_entry_with_tail, read_current_trust_policy_snapshot,
    read_current_trust_policy_snapshot_with_tail,
};

/// One publication-trust issue found during repository verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationTrustIssue {
    /// Stable issue code.
    pub code: &'static str,
    /// Human-readable explanation.
    pub message: String,
    /// The object the issue is about and what its signatures came to, typed (0.57.0 step 3, S3), so a
    /// consumer never has to read the object out of `message`. `None` for an issue about no object
    /// (an unreadable trust policy).
    pub subject: Option<PublicationTrustSubject>,
}

/// The object a [`PublicationTrustIssue`] is about, with one finding per MAINTAINER signature that
/// did not verify as trusted (0.57.0 step 3, S3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationTrustSubject {
    /// The object's type (`block`, `ref-state`, `tag`, `ref-update`, ...).
    pub object_type: prikk_object::ObjectType,
    /// The object's id.
    pub object_id: prikk_object::ObjectId,
    /// Why no signature of it counted: one finding per signature that did not, or one `unsigned`
    /// finding when the object carries no MAINTAINER signature at all.
    pub findings: Vec<PublicationTrustFinding>,
}

/// One MAINTAINER signature that did not count as trusted, or the absence of any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationTrustFinding {
    /// The signature's key id; `None` exactly when the object has no MAINTAINER signature.
    pub key_id: Option<String>,
    /// `unverifiable` (the key is not adopted here), `invalid` (adopted, but the signature does not
    /// verify) or `unsigned` (no MAINTAINER signature at all): the first two are
    /// [`PublicationSignatureOutcome`]'s wire values, the third is only for this case.
    pub outcome: &'static str,
}

impl PublicationTrustIssue {
    /// Construct a publication trust issue about no particular object.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            subject: None,
        }
    }

    /// Construct a publication trust issue about `subject`.
    #[must_use]
    fn about(
        code: &'static str,
        message: impl Into<String>,
        subject: PublicationTrustSubject,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            subject: Some(subject),
        }
    }
}

/// One adopted MAINTAINER key. DC-78 §D5's full TOFU provenance (the block id a key was first
/// accepted at, and the ref name it arrived under) is recorded only for keys adopted through
/// exchange, which lands with the import path — a key declared locally via `trust maintainer add`
/// has no such provenance to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedMaintainerKey {
    /// The adopted maintainer key id.
    pub key_id: String,
    /// The adopted Ed25519 public key.
    pub public_key: [u8; ED25519_KEY_LEN],
}

/// The repository-local set of adopted MAINTAINER keys (DC-78 §D2). A `Block`/`RefState` is trusted
/// if *any* adopted key signed it — object trust, not ref authority; adopting a key never lets it
/// move a ref (`RefStore::publish` still requires a signature from this operator's own signer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintainerTrustPolicy {
    /// Every currently adopted key, in the order it was adopted.
    pub keys: Vec<AdoptedMaintainerKey>,
}

impl MaintainerTrustPolicy {
    pub(crate) fn find(&self, key_id: &str) -> Option<&AdoptedMaintainerKey> {
        self.keys.iter().find(|key| key.key_id == key_id)
    }
}

/// Adopt a MAINTAINER key (DC-78 §D2/§D5): add it if the key id is new; succeed idempotently,
/// changing nothing, if it is already adopted with the *same* public key; **refuse** if it is
/// already adopted with a *different* public key. This is DC-78's TOFU enforcement — "a changed key
/// for a known key id is refused, not re-prompted" — not just the create path for a fresh key id.
/// **TOFU history persists across removal**: the key-material container is never pruned, so
/// re-adopting a key id that was previously removed from the active policy with a *different* public
/// key is still refused — the identity was seen once, and a changed key under a familiar id is exactly
/// what TOFU exists to catch, active policy membership or not. Both refusals are
/// [`PrikkError::MaintainerKeyIdBound`], told apart by `currently_adopted`, so the text (and the CLI's
/// `refusal-v1` id) says whether the id is adopted now or only remembered. Returns the adopted key and
/// whether this call wrote to the *policy*: re-adopting a removed id with its original key returns
/// `true` (the policy gained the id) although the key material was already on record and is not
/// rewritten.
pub fn add_trusted_maintainer(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key_hex: &str,
) -> Result<(AdoptedMaintainerKey, bool)> {
    layout.require_current_format()?;
    let _active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    // RFC 102 Stage 6 Step 2, design-v1.md §15.8: `TrustPolicy` gets its own container-scoped lock,
    // not just `ActiveLock` -- the owner's decision 2 (design-v1.md §15.7) that the exclusion
    // mechanism against the compactor must be container-scoped, so a `prikk compact` run on an
    // unrelated container never contends with this. `ActiveLock` above is unchanged and still serves
    // its original purpose (serializing this op against `seal`/`commit`); this is additional, not a
    // replacement. `trust_key` is deliberately not in this set -- it never compacts, so `ActiveLock`
    // alone remains sufficient for it, same as before this stage.
    let _trust_policy_lock = acquire_container_locks(layout, &[LockableContainer::TrustPolicy])?;
    crate::refs::ensure_no_incomplete_publication(layout)?;
    Signature::validate_key_id(key_id)?;
    validate_maintainer_key_id_storage_safety(key_id)?;
    let public_key = decode_public_key_hex(public_key_hex)?;

    let (mut key_ids, policy_trailing_partial_bytes, policy_tail_offset) =
        current_adopted_key_ids(layout)?;

    // Runs before the key-material lookup, and unconditionally, so a case-insensitive collision can
    // never stand in for this check — see the function's own doc for why it now checks *active
    // policy membership*, not key-material presence (design-v1.md §14.9 §4: a removed key's identity
    // must not go on reserving its case-folded name). Excludes exact self-matches, so a real
    // idempotent or TOFU-refusal re-add of the same key id is unaffected.
    validate_no_maintainer_key_id_collision(&key_ids, key_id)?;

    // RFC 163 §2: the trust-key container's own write-side tail guard, at the one whole read this
    // function already performs before its own conditional `append_trust_key_entry` below -- no
    // second read added.
    let (existing_entry, key_trailing_partial_bytes, key_tail_offset) =
        lookup_trust_key_entry_with_tail(layout, key_id)?;

    // RFC 163 §2 Addendum 2: whether EACH of the two containers will actually be appended to is
    // decided here, before either tail guard runs and before either append -- not one file checked
    // and appended, then the other checked. Addendum 1 item 2 correctly scoped each guard to its own
    // about-to-append branch, but left them interleaved with the writes: a torn tail on the policy
    // container, found only after the trust-key container had already been appended to, left the key
    // recorded but not adopted -- one file changed, the other refused, and the refusal's own text said
    // nothing was written. Deciding both up front closes that: a conflicting key still refuses
    // unconditionally, appending nothing.
    // Whether the id is in the *active* policy now. `existing_entry` below comes from the key-material
    // container, which keeps a removed id's first key as TOFU history, so a conflict can be found for
    // an id that is no longer adopted -- and the refusal must say which of the two it is.
    let currently_adopted = key_ids.iter().any(|existing| existing == key_id);
    let key_will_append = match existing_entry {
        Some(existing) if existing.public_key == public_key => false,
        // RFC 147 §2d: a trust-on-first-use collision, detected before anything is verified -- no
        // signature was checked here, so `InvalidSignature` named the wrong axis. The arm above is
        // idempotent for the same key, which is why `prikk setup` on an existing repository always
        // lands here: fresh material every run, under the fixed key id `maintainer`.
        // 0.56.0 step T: a structural variant, and a message that tells the truth for a removed id
        // (it used to say "already adopted" for one `trust maintainer list` does not show).
        Some(_) => {
            return Err(PrikkError::MaintainerKeyIdBound {
                key_id: key_id.to_string(),
                currently_adopted,
                message: key_id_bound_message(key_id, currently_adopted),
            });
        }
        None => true,
    };
    let policy_will_append = !currently_adopted;

    if key_will_append {
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the trust key container",
            key_trailing_partial_bytes,
            key_tail_offset,
            "back it up, truncate it to the named offset, then run `prikk verify`",
        )?;
    }
    if policy_will_append {
        crate::foundation::tail_guard::require_no_unclean_tail(
            "the trust policy container",
            policy_trailing_partial_bytes,
            policy_tail_offset,
            "back it up, truncate it to the named offset, then run `prikk verify`",
        )?;
    }

    if key_will_append {
        append_trust_key_entry(
            layout,
            &TrustKeyEntry {
                key_id: key_id.to_string(),
                public_key,
            },
        )?;
    }

    let adopted = AdoptedMaintainerKey {
        key_id: key_id.to_string(),
        public_key,
    };
    if !policy_will_append {
        return Ok((adopted, false));
    }
    key_ids.push(key_id.to_string());
    append_trust_policy_snapshot(layout, &key_ids)?;
    Ok((adopted, true))
}

/// The route both causes of [`PrikkError::MaintainerKeyIdBound`] share: adopting the other key under
/// another local id would match none of its blocks, because the id travels in every signature.
const DISTINCT_ID_ROUTE: &str = "A key id travels in every signature, so adopting this key under \
     another local id would match none of its blocks: its maintainer must sign under a distinct id \
     -- a key made by `prikk key generate --out <path>` has one (point PRIKK_MAINTAINER_SEED_FILE \
     at it), or set PRIKK_MAINTAINER_KEY_ID -- and you adopt it under that id with `prikk trust \
     maintainer add --key-id <that id> --public-key <its public key>`";

/// The text of [`PrikkError::MaintainerKeyIdBound`]. The adopted case is the long-standing sentence;
/// the removed case says what is true: the id is not adopted now, but `trust maintainer remove`
/// never forgets which key it first belonged to.
fn key_id_bound_message(key_id: &str, currently_adopted: bool) -> String {
    if currently_adopted {
        format!(
            "maintainer key id {key_id} is already adopted with a different public key. \
             {DISTINCT_ID_ROUTE}"
        )
    } else {
        format!(
            "maintainer key id {key_id} is not currently adopted (it was removed), but it stays \
             bound to the public key it was first adopted with, and a different public key is \
             refused: removing an id takes it out of the adopted set, not out of the record of \
             which key it belongs to. Re-adopting {key_id} with its original public key still \
             works. {DISTINCT_ID_ROUTE}"
        )
    }
}

/// Revoke a MAINTAINER key id: remove it from the active policy by appending a new snapshot without
/// it (design-v1.md §14.9 -- the same operation `add_trusted_maintainer` already performs, just
/// shorter; no tombstone record, no new format concept). The key's own material is never removed from
/// the key-material container -- it stays as TOFU history, so a future re-add with a *different*
/// public key under the same id is still refused. Returns whether the key was actually adopted (a
/// no-op removal of an unadopted id returns `false`, matching `add`'s own idempotent-no-op shape).
/// **Refuses to remove the last adopted key**: an explicitly-empty-but-present policy is a state the
/// old TOML parser could never represent either (`keys = []` was rejected as malformed), and
/// introducing it now would be a new state `verify`'s trust classification does not account for.
pub fn remove_trusted_maintainer(layout: &RepositoryLayout, key_id: &str) -> Result<bool> {
    layout.require_current_format()?;
    let _active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    // See `add_trusted_maintainer`'s identical comment: `TrustPolicy`'s own container-scoped lock,
    // additional to `ActiveLock`, per design-v1.md §15.7/§15.8.
    let _trust_policy_lock = acquire_container_locks(layout, &[LockableContainer::TrustPolicy])?;
    crate::refs::ensure_no_incomplete_publication(layout)?;
    let (mut key_ids, trailing_partial_bytes, tail_offset) = current_adopted_key_ids(layout)?;
    let original_len = key_ids.len();
    key_ids.retain(|existing| existing != key_id);
    if key_ids.len() == original_len {
        return Ok(false);
    }
    if key_ids.is_empty() {
        return Err(PrikkError::Integrity(
            "cannot remove the last trusted maintainer key; a repository policy must not be \
             explicitly empty"
                .to_string(),
        ));
    }
    // RFC 163 §2, Addendum 1 item 2: checked here, not earlier -- a removal that turns out to be a
    // no-op (the id was never adopted) or refused (it is the last key) appends nothing, and must not
    // refuse over a tail it will never write behind.
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the trust policy container",
        trailing_partial_bytes,
        tail_offset,
        "back it up, truncate it to the named offset, then run `prikk verify`",
    )?;
    append_trust_policy_snapshot(layout, &key_ids)?;
    Ok(true)
}

/// The currently-adopted key ids, in adoption order — empty if no policy snapshot has ever been
/// appended (the first key ever adopted in this repository). Distinct from `load_maintainer_trust_
/// policy`'s own "missing policy" error: adding the *first* key in a fresh repository is not a trust
/// failure, so this treats "never configured" and "add" as compatible, while `load_maintainer_trust_
/// policy` (read-only callers, `verify`) must keep treating it as a hard error.
/// RFC 163 §2: also carries the trust-policy container's own tail status, from the same read, for both
/// callers (`add_trusted_maintainer`, `remove_trusted_maintainer`) to check -- **only when each is about
/// to append** (Addendum 1 item 2): a key id already adopted, or a removal that would leave the last
/// key, appends nothing, and must not refuse over a tail it will never write behind.
/// `current_adopted_key_ids` has no other caller (checked from source): every read-only trust-policy
/// question goes through `read_current_trust_policy_snapshot` directly instead, which stays lenient.
fn current_adopted_key_ids(layout: &RepositoryLayout) -> Result<(Vec<String>, usize, usize)> {
    let (snapshot, trailing_partial_bytes, tail_offset) =
        read_current_trust_policy_snapshot_with_tail(layout)?;
    Ok((
        snapshot.unwrap_or_default(),
        trailing_partial_bytes,
        tail_offset,
    ))
}

/// Reject a maintainer key id whose ASCII-folded form collides with a *currently adopted* key id
/// other than itself (DC-72). `key_ids` is the resolved active-policy set, not the key-material
/// container's own full history — design-v1.md §14.9 §4's deliberate behavior change: a removed key
/// no longer reserves its case-folded name. This check's rationale also changed with the container
/// migration: the original hazard was a case-insensitive *filesystem* silently folding two `.pub`
/// filenames onto one (APFS; found on macOS CI after Stage 1 merged —
/// `dc72_path_safety_collisions.rs::maintainer_key_id_rejects_case_insensitive_collision`). A
/// container has no filenames, so that hazard's root cause is gone; this is now a semantic guard
/// against operator confusion and ambiguous audit trails, not a filesystem workaround, which is why
/// its tests were rewritten rather than ported. Folds through `prikk_object::ascii_fold`, the one
/// shared folding definition (DC-72 design ruling, `rfcs/accepted/DC-72-PATH-SAFETY-CONFORMANCE.md`
/// §3.5) — see its doc comment for the recorded NFC/NFD limitation this inherits.
fn validate_no_maintainer_key_id_collision(key_ids: &[String], key_id: &str) -> Result<()> {
    let folded = ascii_fold(key_id);
    for existing_id in key_ids {
        if existing_id != key_id && ascii_fold(existing_id) == folded {
            return Err(PrikkError::InvalidName(format!(
                "case-insensitive maintainer key id collision involving: {existing_id}"
            )));
        }
    }
    Ok(())
}

/// Load and validate the repository-local set of adopted MAINTAINER keys. Errors if no policy
/// snapshot has ever been appended — a repository with no adopted maintainer is a trust failure for
/// every publication, exactly as the old missing-`policy.toml` case was
/// (`PublicationTrustVerifier`'s `PRIKK-TRUST-POLICY-INVALID`).
pub fn load_maintainer_trust_policy(layout: &RepositoryLayout) -> Result<MaintainerTrustPolicy> {
    // RFC 147 §2d / RFC 132's per-site practice: this arm fires on `Ok(None)` -- nothing adopted
    // yet -- which is a caller precondition, not damage. The `?` above already propagates a real
    // read failure, and a damaged snapshot has its own message
    // (`trust_index.rs`'s "damaged snapshot; run doctor before reading"), so `Integrity` here sent
    // every second project looking for corruption instead of running `trust maintainer add`. The
    // parenthetical stays: a container truncated below its own header length replays empty and is
    // indistinguishable from never-adopted at this read, so the message must not claim otherwise.
    let key_ids = read_current_trust_policy_snapshot(layout)?.ok_or_else(|| {
        PrikkError::Precondition(
            "no maintainer key is adopted in this repository yet; run `prikk trust maintainer add` \
             (a trust policy container that replays empty reads the same way -- run `prikk doctor` \
             if a key was adopted here before)"
                .to_string(),
        )
    })?;
    let mut keys = Vec::with_capacity(key_ids.len());
    for key_id in key_ids {
        let entry = lookup_trust_key_entry(layout, &key_id)?.ok_or_else(|| {
            PrikkError::Integrity(format!(
                "trusted maintainer key {key_id} is missing or unreadable"
            ))
        })?;
        keys.push(AdoptedMaintainerKey {
            key_id: entry.key_id,
            public_key: entry.public_key,
        });
    }
    Ok(MaintainerTrustPolicy { keys })
}

/// Load the trust policy for a read-only question ("what does this repository currently trust?"),
/// treating "no policy has ever been adopted" as a valid empty answer rather than
/// [`load_maintainer_trust_policy`]'s hard error. That error is correct for *publication* trust — a
/// Block/RefState needs a definitively trusted signer to be sealed at all — but `prikk trust
/// maintainer list`/`check` (RFC 138) must not refuse a fresh repository that has never adopted
/// anyone; that is exactly the case those commands are asked about most. A `RecognitionClaim`'s own
/// signature check has an identical requirement for an unrelated reason (design D3 rules a claim
/// never gates on trust, so a repository that has never adopted anyone must read every claim's
/// signer as simply not adopted) — both callers share this one definition.
///
/// **Relocated here from `recognition_claim.rs` (RFC 138 carried-defects C).** RFC 130's coupling
/// gate found that leaving it there, and having this module call it by its old address, closed a
/// `trust <-> recognition_claim` cycle that existed for no better reason than this function living
/// in the wrong place. Moving the definition removes the edge outright — `recognition_claim.rs`,
/// `tag_travel.rs`, and `seal_from_accepted.rs` all already depend on `trust` for other reasons and
/// now call this from here instead of from `recognition_claim`.
///
/// A genuinely damaged trust container still propagates its error unchanged — only the "nothing has
/// ever been adopted" case is treated as empty.
pub fn load_maintainer_trust_policy_or_empty(
    layout: &RepositoryLayout,
) -> Result<MaintainerTrustPolicy> {
    match read_current_trust_policy_snapshot(layout)? {
        Some(_) => load_maintainer_trust_policy(layout),
        None => Ok(MaintainerTrustPolicy { keys: Vec::new() }),
    }
}

/// RFC 118 stage 3: the declared set of operations that gate on [`verify_signer_trusted`] -- one
/// variant per distinct publishing act, not one per call site (`Seal` covers both `seal.rs` call
/// sites: the ordinary path and the signer-backed recovery path for an already-matching WAL, since
/// both publish the same class of object under the same terms, just entering from a different
/// precondition). This is the single source `trust-threat-model.md`'s gated-operation list derives
/// from (`trust_gated_operations_binding_gate.rs`) -- adding a variant here without naming it in
/// that page fails the binding gate, and naming an operation in the page that has no variant here
/// fails it the other way. **This enum states which operations gate; it does not, and cannot, state
/// that every operation which *ought* to gate does** -- see the page's own note on that distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatedOperation {
    /// `prikk seal` (`crates/prikk-cli/src/seal.rs`), both the ordinary and signer-backed-recovery
    /// paths.
    Seal,
    /// `prikk merge` (`crates/prikk-store/src/merge_execute.rs::execute_merge`).
    Merge,
    /// `prikk sync build` (`crates/prikk-store/src/sync_negotiation/sender.rs::build_sync_artifact`).
    SyncBuild,
    /// `prikk sync seal` (`crates/prikk-store/src/seal_from_accepted.rs::seal_from_accepted_claim`).
    SyncSeal,
    /// `prikk sync adopt-tag` (`crates/prikk-store/src/tag_travel.rs::adopt_tag`).
    SyncAdoptTag,
    /// `prikk tag create` (`crates/prikk-cli/src/tag.rs`).
    TagCreate,
    /// `prikk branch create` (`crates/prikk-cli/src/branch.rs::run_create`).
    BranchCreate,
    /// `prikk branch close` (`crates/prikk-cli/src/branch.rs::run_close`).
    BranchClose,
    /// `prikk ref complete` (RFC 165 R4, `crates/prikk-store/src/ref_completion.rs::
    /// complete_ref_publication`) -- owner decision 2 (RFC 165 §6): any adopted maintainer key may
    /// complete another's interrupted publication, so this gates the *completing* signer, separate
    /// from [`verify_trusted_publication_envelope`]'s check of the *original* signer inside
    /// `plan_ref_completion`'s own condition (a).
    RefComplete,
    /// `prikk bundle import --adopt` (0.57.0 S1b, `bundle/adopt.rs`): the receiver signs the `RefUpdate`s that publish an adopted
    /// delivered branch, so its key is gated like every other publisher's.
    BundleAdopt,
}

/// Verify that the signer matches one of the repository-local trust policy's adopted keys.
///
/// `operation` is accepted, not read: every call site must name which operation it is performing
/// (RFC 118 stage 3 §3), which the compiler enforces since the parameter is required, but this
/// stage does not change any refusal message (out of scope, §5) -- an operation-aware message
/// (e.g. "tag create refused: signer not trusted") is a real, separate improvement, reported as a
/// follow-up rather than built here.
pub fn verify_signer_trusted(
    layout: &RepositoryLayout,
    signer: &impl MaintainerSigner,
    _operation: GatedOperation,
) -> Result<MaintainerTrustPolicy> {
    let policy = load_maintainer_trust_policy(layout)?;
    // RFC 147 §2d: policy *membership*, tested before any signature is verified -- the exported key
    // id is simply not among the adopted ones. The public-key comparison below is a different
    // question and keeps `InvalidSignature`: there the id matched and the material did not.
    let Some(matched) = policy.find(signer.key_id()) else {
        return Err(PrikkError::Precondition(format!(
            "maintainer signer key id {} is not trusted by policy; run `prikk trust maintainer \
             add` for it, or export a key id this repository already trusts",
            signer.key_id()
        )));
    };
    let signer_public_key = signer.public_key_bytes();
    if signer_public_key != matched.public_key {
        return Err(PrikkError::InvalidSignature(format!(
            "maintainer signer public key does not match trusted key {}",
            matched.key_id
        )));
    }
    Ok(policy)
}

/// **The MAINTAINER key ids an object's envelope records**, as recorded, in the order stored: the one
/// definition of `sealed_by` (0.57.0 step 3, S2). `bundle preview` reads it from the exported RefState,
/// `log --format json` from each Block -- the same function, so the two cannot differ in what "the
/// key ids a seal records" means.
///
/// **Recorded, not trusted.** This reads the envelope's own signature list and nothing else: not the
/// adopted policy, not whether a signature verifies. So it reads the same before and after
/// `trust maintainer remove` of a sealer (adoption is local, and a key can be removed after it
/// signed), which is the whole point -- a consumer asking "who sealed this" must not get a different
/// answer from a trust edit. Which of these signatures this repository trusts is a different question
/// (`verify`, and `bundle preview`'s `sealed_by_signatures`).
#[must_use]
pub fn recorded_maintainer_key_ids(envelope: &ObjectEnvelope) -> Vec<String> {
    envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id.clone())
        .collect()
}

/// Verify a publication envelope against the current repository-local trust policy. Returns the
/// adopted key id whose signature matched (DC-78 §D3), the first in canonical order: the sealer's
/// identity already lives inside the envelope's own signature, non-strippably — this is reporting that
/// fact, not new state.
///
/// **Every** MAINTAINER signature made by an adopted key is checked (RFC 156 Stage 1): one that does not
/// verify is `PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE`, even when another adopted signature does. A
/// signature by a key this repository has not adopted cannot be checked here and is not, by itself, an
/// issue — adoption is local, and a key can be removed after it signed.
pub fn verify_trusted_publication_envelope(
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
) -> std::result::Result<String, PublicationTrustIssue> {
    verify_trusted_publication_envelope_with_outcomes(policy, envelope).0
}

/// [`verify_trusted_publication_envelope`]'s verdict **and** every MAINTAINER signature's own outcome,
/// from one pass: each adopted signature is verified once and the result serves both (0.57.0 step 3, S3:
/// `verify`'s per-block `sealed_by_signatures` costs no second Ed25519 verification). The verdict is
/// unchanged: any adopted signature that does not verify is `INVALID-SIGNATURE` (the first, in the
/// envelope's order), else the first one that does is the trusted key, else `UNTRUSTED`.
pub fn verify_trusted_publication_envelope_with_outcomes(
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
) -> (
    std::result::Result<String, PublicationTrustIssue>,
    Vec<(String, PublicationSignatureOutcome)>,
) {
    let object_id = envelope.object_id();
    let mut outcomes = Vec::new();
    let mut trusted: Option<String> = None;
    let mut first_invalid: Option<(String, PrikkError)> = None;
    for signature in envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
    {
        let outcome = if policy.find(&signature.key_id).is_none() {
            PublicationSignatureOutcome::Unverifiable
        } else {
            match verify_trusted_signature(policy, envelope, signature, object_id) {
                Ok(()) => {
                    if trusted.is_none() {
                        trusted = Some(signature.key_id.clone());
                    }
                    PublicationSignatureOutcome::Sound
                }
                Err(err) => {
                    if first_invalid.is_none() {
                        first_invalid = Some((signature.key_id.clone(), err));
                    }
                    PublicationSignatureOutcome::Invalid
                }
            }
        };
        outcomes.push((signature.key_id.clone(), outcome));
    }
    if let Some((key_id, err)) = first_invalid {
        let issue = PublicationTrustIssue::about(
            "PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE",
            format!(
                "{} {} carries a MAINTAINER signature by adopted key {} that does not \
                 verify: {err}",
                envelope.object_type, object_id, key_id
            ),
            PublicationTrustSubject {
                object_type: envelope.object_type,
                object_id,
                findings: vec![PublicationTrustFinding {
                    key_id: Some(key_id),
                    outcome: PublicationSignatureOutcome::Invalid.as_str(),
                }],
            },
        );
        return (Err(issue), outcomes);
    }
    let verdict = trusted.ok_or_else(|| {
        // Every MAINTAINER signature here is by a key this repository has not adopted (an adopted one
        // either verified, which made it trusted, or failed, returned above as invalid): one
        // `unverifiable` finding each, or one `unsigned` finding when there is none.
        let mut findings: Vec<PublicationTrustFinding> = outcomes
            .iter()
            .map(|(key_id, outcome)| PublicationTrustFinding {
                key_id: Some(key_id.clone()),
                outcome: outcome.as_str(),
            })
            .collect();
        if findings.is_empty() {
            findings.push(PublicationTrustFinding {
                key_id: None,
                outcome: "unsigned",
            });
        }
        PublicationTrustIssue::about(
            "PRIKK-TRUST-PUBLICATION-UNTRUSTED",
            format!(
                "{} {} has no trusted MAINTAINER signature",
                envelope.object_type, object_id
            ),
            PublicationTrustSubject {
                object_type: envelope.object_type,
                object_id,
                findings,
            },
        )
    });
    (verdict, outcomes)
}

/// 0.56.0 step F: turn [`verify_trusted_publication_envelope`]'s own verdict into an admission
/// decision for an importer (stikk 023 part 6). `PRIKK-TRUST-PUBLICATION-UNTRUSTED` (no adopted
/// key signed at all) is left exactly as every importer already treats it today -- `Ok(())`, so a
/// sealer this repository has not adopted still imports, unchanged. Only
/// `PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE` -- a signature naming an adopted key id that does
/// not verify against it -- becomes a hard refusal: a forgery, or another maintainer using the
/// same key id. Classified by `PublicationTrustIssue::code`, never by matching its own message
/// text, the same discipline `check_signatures` (`archive/verify.rs`) already follows for the
/// identical two codes.
///
/// **One shared check, not a new copy:** every caller (`bundle::import_bundle`,
/// `archive::import::land_one_ref`, `archive::import`'s own fail-fast gate) calls this directly,
/// rather than re-deriving the `issue.code` match at its own call site.
///
/// **0.56.0 F2 item 2** (review `.git-exclude/reviewed/0.56.0-step-3-f-review-v1.md`, "F2 item
/// 2"): the message named two possible causes -- a forgery, or another maintainer using the same
/// key id -- but gave no route out for the second, entirely legitimate one. Two independent,
/// pre-0.45.0 "legacy" installations that both happen to default to the shared maintainer key id
/// `"maintainer"` now correctly refuse here at import instead of importing cleanly and failing a
/// later `verify` -- right, but a dead end without a route. The route added below mirrors
/// `add_trusted_maintainer`'s own TOFU-collision refusal verbatim (same function below, "already
/// adopted with a different public key"): the *sending* maintainer signs under a distinct key id
/// next time (`PRIKK_MAINTAINER_KEY_ID`, or a key made by `prikk key generate --out <path>`,
/// pointed at by `PRIKK_MAINTAINER_SEED_FILE`), and the *receiving* repository adopts that
/// distinct id with `prikk trust maintainer add --key-id <that id> --public-key <its public
/// key>`. Proven end to end by `distinct_default_key_ids.rs::
/// control7_the_forged_publication_signature_refusal_at_import_names_a_route_that_runs`, the
/// identical round-trip `control6_the_maintainer_refusal_names_a_route_that_runs`'s own "Route 1"
/// already proves for the sibling `trust maintainer add` collision.
pub fn refuse_forged_publication_signature(
    command: &'static str,
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
) -> Result<()> {
    match verify_trusted_publication_envelope(policy, envelope) {
        Ok(_) => Ok(()),
        Err(issue) if issue.code == "PRIKK-TRUST-PUBLICATION-UNTRUSTED" => Ok(()),
        Err(issue) => Err(PrikkError::ForgedPublicationSignature {
            command,
            message: format!(
                "{} -- a forgery, or another maintainer using the same key id; if it is the \
                 latter, that maintainer should sign under a distinct id next time -- a key made \
                 by `prikk key generate --out <path>` has one (point PRIKK_MAINTAINER_SEED_FILE \
                 at it), or set PRIKK_MAINTAINER_KEY_ID -- and you adopt it under that id with \
                 `prikk trust maintainer add --key-id <that id> --public-key <its public key>`; \
                 refusing before admitting it",
                issue.message
            ),
        }),
    }
}

/// 0.56.0 step F: this envelope's own per-MAINTAINER-signature publication-trust classification,
/// one entry per signature, in the envelope's own signature order -- `sound` (adopted, verifies),
/// `unverifiable` (not adopted by this repository at all), or `invalid` (adopted, but does not
/// verify -- a forgery, or another maintainer using the same key id). `bundle preview`'s own
/// `sealed_by_signatures` needs each signature's own outcome; `verify_trusted_publication_envelope`'s
/// own aggregate answer only ever says whether *some* signature is trusted, or names the first
/// failure, which collapses several sealers into one verdict. Reuses `verify_trusted_signature`,
/// the identical per-signature Ed25519 check `verify_trusted_publication_envelope` and
/// `crate::recognition_claim::verify_claim_signature` already call, rather than a third copy of
/// that logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PublicationSignatureOutcome {
    /// Adopted, and verifies.
    Sound,
    /// Adopted, but does not verify -- a forgery, or another maintainer using the same key id.
    Invalid,
    /// Not adopted by this repository at all; not checked.
    Unverifiable,
}

impl PublicationSignatureOutcome {
    /// Stable, lowercase label -- `bundle preview --format json`'s own wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sound => "sound",
            Self::Invalid => "invalid",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// Every MAINTAINER signature on `envelope`, classified by [`PublicationSignatureOutcome`].
#[must_use]
pub fn classify_publication_signatures(
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
) -> Vec<(String, PublicationSignatureOutcome)> {
    let object_id = envelope.object_id();
    envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| {
            let outcome = if policy.find(&signature.key_id).is_none() {
                PublicationSignatureOutcome::Unverifiable
            } else {
                match verify_trusted_signature(policy, envelope, signature, object_id) {
                    Ok(()) => PublicationSignatureOutcome::Sound,
                    Err(_) => PublicationSignatureOutcome::Invalid,
                }
            };
            (signature.key_id.clone(), outcome)
        })
        .collect()
}

/// 0.56.0 step 3i: a carried maintainer key's classification against the *receiving* repository's
/// own adopted trust policy -- never the archive's own carried policy
/// (`archive::verify::verify_archive` keeps reporting *"internally consistent"*, never
/// *"trusted"*; this is the receiver-side join that module's own doc comment says belongs outside
/// it). The planeter letter's fourth case ("carried, not checked against a repository" -- outside
/// any repository, so there is no policy at all) has no counterpart here and is decided at the CLI
/// layer before this is ever called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CarriedKeyRepositoryStatus {
    /// No entry under this key id in the receiving repository's adopted policy.
    NotAdopted,
    /// An entry under this key id, with the identical public key.
    Adopted,
    /// An entry under this key id, with a *different* public key -- the same key id adopted here
    /// under different key material.
    Conflicts,
}

impl CarriedKeyRepositoryStatus {
    /// Stable, lowercase label -- the planeter letter's own fixed `status` string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotAdopted => "carried, not adopted",
            Self::Adopted => "adopted",
            Self::Conflicts => "carried, conflicts with an adopted key",
        }
    }
}

/// Classify one carried key id/public key (as `archive::verify::CarriedMaintainerKeySummary`
/// already holds them, lowercase hex) against `policy`. Reuses the identical `policy.find` +
/// public-key comparison shape `add_trusted_maintainer`'s own TOFU-collision check already uses
/// (this function's own doc, "already adopted with a different public key") -- not a new
/// comparison mechanism. Re-encodes the adopted key's raw bytes with the same `prikk_hash::to_hex`
/// archive verification already uses, rather than decoding `public_key_hex` back to bytes, so the
/// one comparison this function makes stays in one representation.
#[must_use]
pub fn classify_carried_key_against_repository(
    policy: &MaintainerTrustPolicy,
    key_id: &str,
    public_key_hex: &str,
) -> CarriedKeyRepositoryStatus {
    match policy.find(key_id) {
        None => CarriedKeyRepositoryStatus::NotAdopted,
        Some(existing) if prikk_hash::to_hex(&existing.public_key) == public_key_hex => {
            CarriedKeyRepositoryStatus::Adopted
        }
        Some(_) => CarriedKeyRepositoryStatus::Conflicts,
    }
}

pub(crate) fn verify_trusted_signature(
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
    signature: &Signature,
    object_id: prikk_object::ObjectId,
) -> Result<()> {
    if signature.algorithm != SignatureAlgorithm::Ed25519 {
        return Err(PrikkError::InvalidSignature(
            "publication signature is not Ed25519".to_string(),
        ));
    }
    if signature.signer_role != SignerRole::Maintainer {
        return Err(PrikkError::InvalidSignature(
            "publication signature role is not MAINTAINER".to_string(),
        ));
    }
    // O(adopted keys) string comparisons to find which key this signature claims, then at most one
    // Ed25519 verification (DC-78 §D7.4) — never one verification per adopted key.
    let Some(matched) = policy.find(&signature.key_id) else {
        return Err(PrikkError::InvalidSignature(
            "publication signature key id is not trusted".to_string(),
        ));
    };
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        envelope.object_type,
        object_id,
        SignerRole::Maintainer,
        &signature.key_id,
    )?;
    verify_ed25519(&matched.public_key, &preimage, &signature.signature_bytes)
}

fn decode_public_key_hex(hex: &str) -> Result<[u8; ED25519_KEY_LEN]> {
    if hex.len() != ED25519_KEY_LEN * 2 {
        return Err(PrikkError::MalformedData(format!(
            "maintainer public key must be {} lowercase hex characters",
            ED25519_KEY_LEN * 2
        )));
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PrikkError::MalformedData(
            "maintainer public key contains non-hex bytes".to_string(),
        ));
    }
    if hex.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(PrikkError::MalformedData(
            "maintainer public key must use lowercase hex".to_string(),
        ));
    }
    let mut out = [0_u8; ED25519_KEY_LEN];
    for (slot, pair) in out.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let hi =
            hex_value(pair.first().copied().ok_or_else(|| {
                PrikkError::MalformedData("truncated public key hex".to_string())
            })?)?;
        let lo =
            hex_value(pair.get(1).copied().ok_or_else(|| {
                PrikkError::MalformedData("truncated public key hex".to_string())
            })?)?;
        *slot = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(PrikkError::MalformedData(
            "maintainer public key must use lowercase hex".to_string(),
        )),
    }
}

// Repository mutation is implemented on Linux, macOS and Windows (`platform-support.md`); the
// test that injects a directory sync carries its own narrower gate.
#[cfg(test)]
mod tests;
