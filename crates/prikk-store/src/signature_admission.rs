//! Which of an incoming envelope's signatures may join an object this repository already stores
//! (RFC 156 §4, rules 1–3), decided at the door — `bundle import` and `sync accept` — before anything is
//! written.
//!
//! The store's own write decision (`foundation/index.rs`) merges whatever it is given: `foundation` sits
//! below `trust` and the author-key index, so it cannot judge a signature. Local writers only ever add a
//! signature they just made with a configured key. Untrusted signatures arrive through imports, so the
//! rules are enforced here, for every signature an import would add to a stored object:
//!
//! 1. **Every signature stored is verified first.** One that names a key this repository can check and
//!    does not verify refuses the whole import, naming the object and the key — nothing is written.
//! 2. **A MAINTAINER signature is stored only if an adopted key made it.** Any other is dropped and
//!    reported: trust comes only from adopted keys, so it adds nothing a reader can act on.
//! 3. **An AUTHOR signature is stored if it verifies against its key id's material** — recorded here, or
//!    carried by the import itself (trust on first use, the binding rules unchanged). With no material
//!    anywhere it cannot be verified, so under rule 1 it cannot be stored: it is dropped and reported.
//!
//! An object the repository does not hold yet is not a union, and imports keep storing it as carried,
//! exactly as before: import records, `verify` decides.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, SignerRole};

use crate::author::author_key_index::{AuthorKeyEntry, verify_one_author_signature};
use crate::trust::{MaintainerTrustPolicy, verify_trusted_signature};

/// Why a signature was not stored.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroppedSignatureReason {
    /// A MAINTAINER signature by a key this repository has not adopted (RFC 156 §4 rule 2).
    MaintainerKeyNotAdopted,
    /// An AUTHOR signature whose key id has no recorded or carried material, so it cannot be verified
    /// (RFC 156 §4 rules 1 and 3).
    AuthorKeyUnrecorded,
    /// A CI or AUDIT signature: prikk records no key material and adopts no key for either role, so it
    /// can never be verified here (RFC 156 §4 rule 1).
    RoleNotVerifiable,
}

impl DroppedSignatureReason {
    /// The stable sentence a report prints.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MaintainerKeyNotAdopted => "its maintainer key is not adopted here",
            Self::AuthorKeyUnrecorded => {
                "no key material for its author key id, so it cannot be verified"
            }
            Self::RoleNotVerifiable => "prikk verifies no signature of this role",
        }
    }
}

/// One signature an import carried for an object already stored here, and did not store.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedSignature {
    /// The object the signature was for.
    pub object_type: ObjectType,
    /// Its id.
    pub object_id: ObjectId,
    /// The signature's role.
    pub signer_role: SignerRole,
    /// The key id it named.
    pub key_id: String,
    /// Why it was not stored.
    pub reason: DroppedSignatureReason,
}

/// What an import will write for one object it carries that this repository already stores.
pub(crate) struct Admission {
    /// The envelope to hand the store: the stored record plus every admitted signature. The store's own
    /// decision then writes the union, or nothing when no signature was admitted.
    pub(crate) envelope: ObjectEnvelope,
    /// Whether any signature was admitted.
    pub(crate) adds_signatures: bool,
    /// Signatures not stored, and why.
    pub(crate) dropped: Vec<DroppedSignature>,
}

/// Decide which of `incoming`'s signatures may join `stored` (same id).
///
/// `author_material` answers the entries this import may verify an AUTHOR key id against: this
/// repository's recorded entries plus the import's own carried ones. When the two envelopes differ in
/// more than signatures, nothing is admitted and `incoming` is returned unchanged, so the store's own
/// decision refuses it as the corruption it is.
pub(crate) fn admit_signatures(
    stored: &ObjectEnvelope,
    incoming: &ObjectEnvelope,
    policy: &MaintainerTrustPolicy,
    mut author_material: impl FnMut(&str) -> Result<Vec<AuthorKeyEntry>>,
) -> Result<Admission> {
    if stored.object_type != incoming.object_type
        || stored.schema_version != incoming.schema_version
        || stored.canonical_payload != incoming.canonical_payload
    {
        return Ok(Admission {
            envelope: incoming.clone(),
            adds_signatures: false,
            dropped: Vec::new(),
        });
    }
    let object_id = incoming.object_id();
    let mut union = stored.clone();
    let mut adds_signatures = false;
    let mut dropped = Vec::new();
    for signature in &incoming.signatures {
        if stored
            .signatures
            .binary_search_by(|existing| existing.canonical_cmp(signature))
            .is_ok()
        {
            continue;
        }
        let drop_reason = match signature.signer_role {
            SignerRole::Maintainer => {
                if policy.find(&signature.key_id).is_none() {
                    Some(DroppedSignatureReason::MaintainerKeyNotAdopted)
                } else {
                    verify_trusted_signature(policy, incoming, signature, object_id).map_err(
                        |err| {
                            PrikkError::InvalidSignature(format!(
                                "{} {object_id} carries a MAINTAINER signature by adopted key {} \
                                 that does not verify ({err}) -- refusing the whole import, nothing \
                                 was written",
                                incoming.object_type, signature.key_id
                            ))
                        },
                    )?;
                    None
                }
            }
            SignerRole::Author => {
                let entries = author_material(&signature.key_id)?;
                if entries.is_empty() {
                    Some(DroppedSignatureReason::AuthorKeyUnrecorded)
                } else {
                    verify_one_author_signature(incoming, signature, &entries)?;
                    None
                }
            }
            SignerRole::Ci | SignerRole::Audit => Some(DroppedSignatureReason::RoleNotVerifiable),
        };
        match drop_reason {
            Some(reason) => dropped.push(DroppedSignature {
                object_type: incoming.object_type,
                object_id,
                signer_role: signature.signer_role,
                key_id: signature.key_id.clone(),
                reason,
            }),
            None => {
                union.add_signature(signature.clone())?;
                adds_signatures = true;
            }
        }
    }
    Ok(Admission {
        envelope: union,
        adds_signatures,
        dropped,
    })
}
