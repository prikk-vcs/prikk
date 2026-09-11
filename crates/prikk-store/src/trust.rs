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
    read_current_trust_policy_snapshot,
};

/// One publication-trust issue found during repository verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationTrustIssue {
    /// Stable issue code.
    pub code: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

impl PublicationTrustIssue {
    /// Construct a publication trust issue.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
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
    fn find(&self, key_id: &str) -> Option<&AdoptedMaintainerKey> {
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
/// what TOFU exists to catch, active policy membership or not. Returns the adopted key and whether
/// this call actually wrote anything.
pub fn add_trusted_maintainer(
    layout: &RepositoryLayout,
    key_id: &str,
    public_key_hex: &str,
) -> Result<(AdoptedMaintainerKey, bool)> {
    layout.require_current_format()?;
    let _active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
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

    let mut key_ids = current_adopted_key_ids(layout)?;

    // Runs before the key-material lookup, and unconditionally, so a case-insensitive collision can
    // never stand in for this check — see the function's own doc for why it now checks *active
    // policy membership*, not key-material presence (design-v1.md §14.9 §4: a removed key's identity
    // must not go on reserving its case-folded name). Excludes exact self-matches, so a real
    // idempotent or TOFU-refusal re-add of the same key id is unaffected.
    validate_no_maintainer_key_id_collision(&key_ids, key_id)?;

    match lookup_trust_key_entry(layout, key_id)? {
        Some(existing) if existing.public_key == public_key => {}
        // RFC 147 §2d: a trust-on-first-use collision, detected before anything is verified -- no
        // signature was checked here, so `InvalidSignature` named the wrong axis. The arm above is
        // idempotent for the same key, which is why `prikk setup` on an existing repository always
        // lands here: fresh material every run, under the fixed key id `maintainer`.
        Some(_) => {
            return Err(PrikkError::Precondition(format!(
                "maintainer key id {key_id} is already adopted with a different public key; adopt \
                 the new key under a different id, or export the seed already adopted under this \
                 one"
            )));
        }
        None => {
            append_trust_key_entry(
                layout,
                &TrustKeyEntry {
                    key_id: key_id.to_string(),
                    public_key,
                },
            )?;
        }
    }

    let adopted = AdoptedMaintainerKey {
        key_id: key_id.to_string(),
        public_key,
    };
    if key_ids.iter().any(|existing| existing == key_id) {
        return Ok((adopted, false));
    }
    key_ids.push(key_id.to_string());
    append_trust_policy_snapshot(layout, &key_ids)?;
    Ok((adopted, true))
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
    let _active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    // See `add_trusted_maintainer`'s identical comment: `TrustPolicy`'s own container-scoped lock,
    // additional to `ActiveLock`, per design-v1.md §15.7/§15.8.
    let _trust_policy_lock = acquire_container_locks(layout, &[LockableContainer::TrustPolicy])?;
    crate::refs::ensure_no_incomplete_publication(layout)?;
    let mut key_ids = current_adopted_key_ids(layout)?;
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
    append_trust_policy_snapshot(layout, &key_ids)?;
    Ok(true)
}

/// The currently-adopted key ids, in adoption order — empty if no policy snapshot has ever been
/// appended (the first key ever adopted in this repository). Distinct from `load_maintainer_trust_
/// policy`'s own "missing policy" error: adding the *first* key in a fresh repository is not a trust
/// failure, so this treats "never configured" and "add" as compatible, while `load_maintainer_trust_
/// policy` (read-only callers, `verify`) must keep treating it as a hard error.
fn current_adopted_key_ids(layout: &RepositoryLayout) -> Result<Vec<String>> {
    Ok(read_current_trust_policy_snapshot(layout)?.unwrap_or_default())
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

/// Verify a publication envelope against the current repository-local trust policy. Returns the
/// adopted key id whose signature matched (DC-78 §D3): the sealer's identity already lives inside
/// the envelope's own signature, non-strippably — this is reporting that fact, not new state.
pub fn verify_trusted_publication_envelope(
    policy: &MaintainerTrustPolicy,
    envelope: &ObjectEnvelope,
) -> std::result::Result<String, PublicationTrustIssue> {
    let object_id = envelope.object_id();
    envelope
        .signatures
        .iter()
        .find_map(|signature| {
            verify_trusted_signature(policy, envelope, signature, object_id)
                .ok()
                .map(|()| signature.key_id.clone())
        })
        .ok_or_else(|| {
            PublicationTrustIssue::new(
                "PRIKK-TRUST-PUBLICATION-UNTRUSTED",
                format!(
                    "{} {} has no trusted MAINTAINER signature",
                    envelope.object_type, object_id
                ),
            )
        })
}

fn verify_trusted_signature(
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

// DC-71: every test here sets up its scenario via real repository mutation (RepositoryLayout::init
// or equivalent), which is Linux-only; the module never compiles a non-Linux-meaningful test.
#[cfg(all(test, target_os = "linux"))]
mod tests;
