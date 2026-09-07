//! Author signing boundary for node-addressed worktree authoring (DC-09 Phase 4.4a, R1).
//!
//! Authoring no longer attaches a placeholder signature. It builds the role-bound preimage via
//! [`prikk_object::Signature::signed_bytes`] over the unsigned patch object id and delegates the
//! actual signing to an injected [`AuthorSigner`]. The production implementation
//! ([`Ed25519AuthorSigner`]) produces a real Ed25519 signature through `prikk-crypto`; tests inject a
//! deterministic Ed25519 signer. There is no placeholder signer on the production Patch AUTHOR
//! signing paths; rollback drafts use this same boundary after marking their payload purpose.
//!
//! Out of scope here (later phases): trust stores, key persistence, rotation, and signature policy.

use prikk_crypto::Ed25519KeyPair;
use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType, Signature, SignatureAlgorithm, SignerRole};

/// A provider that produces the detached signature bytes for an authored patch.
///
/// The authoring engine builds the role-bound preimage and calls [`AuthorSigner::sign`]; the signer
/// owns the secret key material and the key identifier recorded in the resulting [`Signature`].
pub trait AuthorSigner {
    /// The non-empty key identifier recorded in the produced signature.
    fn key_id(&self) -> &str;
    /// Produce the detached signature bytes over the role-bound `preimage`.
    fn sign(&self, preimage: &[u8]) -> Result<Vec<u8>>;
    /// The public key half of this signer's key material (DC-53 Stage 1). The signer is the only
    /// party that holds it, so recording AUTHOR key material at authoring time -- the mechanism
    /// verification depends on, since Ed25519 signatures are not public-key-recoverable -- requires
    /// it from here, not from anything persisted in the `Signature` itself (`key_id` is a label, not
    /// key material).
    fn public_key_bytes(&self) -> [u8; prikk_crypto::ED25519_KEY_LEN];
}

/// Build a role-bound AUTHOR [`Signature`] for the unsigned patch `object_id` using `signer`.
///
/// The preimage binds algorithm (`Ed25519`), object type (`Patch`), the object id, the signer role
/// (`Author`), and the signer's key id, exactly as verification reconstructs it. This is the single
/// author-signing entry point used by node-addressed worktree/commit authoring.
pub fn author_signature(signer: &impl AuthorSigner, object_id: ObjectId) -> Result<Signature> {
    let preimage = Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        object_id,
        SignerRole::Author,
        signer.key_id(),
    )?;
    let signature_bytes = signer.sign(&preimage)?;
    let signature = Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: signer.key_id().to_string(),
        signature_bytes,
        // Advisory only (never used as authoritative audit time); fixed for reproducible identity.
        created_at: 0,
        signer_role: SignerRole::Author,
    };
    signature.validate()?;
    signature.validate_shape()?;
    Ok(signature)
}

/// Production AUTHOR signer: a real Ed25519 keypair plus a caller-provided key id.
pub struct Ed25519AuthorSigner {
    key_id: String,
    keypair: Ed25519KeyPair,
}

impl Ed25519AuthorSigner {
    /// Construct from a non-empty key id and an Ed25519 keypair.
    pub fn new(key_id: impl Into<String>, keypair: Ed25519KeyPair) -> Result<Self> {
        let key_id = key_id.into();
        Signature::validate_key_id(&key_id)?;
        Ok(Self { key_id, keypair })
    }

    /// Construct from a non-empty key id and a 32-byte secret seed (caller-provided key material).
    ///
    /// # Examples
    ///
    /// The key id is validated here, at construction -- not deferred to the first `sign` call or
    /// to whatever later tries to read it back off a signed envelope. A caller who typos a key id
    /// with, say, a colon in it finds out immediately, rather than authoring a real patch first and
    /// only failing when it is signed or replayed.
    ///
    /// ```
    /// use prikk_store::Ed25519AuthorSigner;
    ///
    /// let signer = Ed25519AuthorSigner::from_seed("dev-author", &[1u8; 32]);
    /// assert!(signer.is_ok());
    ///
    /// let invalid = Ed25519AuthorSigner::from_seed("dev:author", &[1u8; 32]);
    /// assert!(invalid.is_err(), "a key id with a colon must be refused, not accepted silently");
    /// ```
    pub fn from_seed(
        key_id: impl Into<String>,
        seed: &[u8; prikk_crypto::ED25519_KEY_LEN],
    ) -> Result<Self> {
        Self::new(key_id, Ed25519KeyPair::from_seed(seed))
    }
}

impl AuthorSigner for Ed25519AuthorSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn sign(&self, preimage: &[u8]) -> Result<Vec<u8>> {
        Ok(self.keypair.sign(preimage).to_vec())
    }

    fn public_key_bytes(&self) -> [u8; prikk_crypto::ED25519_KEY_LEN] {
        self.keypair.public_key_bytes()
    }
}
