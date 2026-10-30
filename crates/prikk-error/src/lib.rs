#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Shared error taxonomy for Prikk crates.

use core::fmt;

/// Shared result type.
pub type Result<T> = core::result::Result<T, PrikkError>;

/// Error type used by the initial implementation crates.
///
/// `#[non_exhaustive]` (RFC 132 increment 1): `prikk-error` is published, and until this attribute
/// landed, adding any new variant was a breaking change for every downstream match. Verified free to
/// add before landing it: no exhaustive `match` on a `PrikkError` value exists anywhere in this
/// workspace.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrikkError {
    /// Canonical encoding failed because input data violates the frozen schema contract.
    CanonicalEncoding(String),
    /// An object identifier had an invalid form.
    InvalidObjectId(String),
    /// A signature had an invalid form or did not match its envelope context.
    InvalidSignature(String),
    /// A path-like name failed Prikk path/ref validation.
    InvalidName(String),
    /// A persistent object had an unexpected type.
    ObjectTypeMismatch {
        /// The object type required by the caller.
        expected: String,
        /// The object type actually found in the stored envelope.
        actual: String,
    },
    /// The persistent format version is unsupported.
    UnsupportedFormatVersion(u32),
    /// A persisted object or record has malformed bytes.
    MalformedData(String),
    /// A persisted object was found at a path that does not match its computed ID.
    Integrity(String),
    /// A lock could not be acquired because another writer may be active.
    LockConflict(String),
    /// A caller precondition for the requested operation is not satisfied. Distinct from
    /// [`Self::LockConflict`], which means another writer may hold a lock: nothing here is
    /// transient and waiting does not help — the caller must change what they asked for.
    Precondition(String),
    /// 019 §5.2/Part B review: repository mutation is blocked because a ref's own pointer and the
    /// ref log disagree -- the one condition a writer's generic [`Self::Precondition`] used to
    /// cover for this specific case, until classifying it by matching the message's own words (so
    /// an edit to the text could silently drop the classification) was ruled out. Its own `Display`
    /// text is unchanged from `Precondition`'s; only callers that need to tell this condition apart
    /// from every other precondition -- to decide whether `prikk ref complete <ref>` is the way out
    /// -- match the variant instead of the words.
    IncompletePublication(String),
    /// Handoff 165 Q1b: a compacting container's generation log is lost, and its two slots fit two
    /// genuinely different, equally honest histories -- not damage, and not a state content can
    /// decide (Q1's own proof: two command sequences that disagree on a revoked key's trust leave
    /// byte-identical slots). Distinct from [`Self::Integrity`] so a caller can tell "guessing would
    /// be wrong either way" apart from "a record is corrupt" by matching the variant, the same
    /// reason [`Self::IncompletePublication`] exists apart from [`Self::Precondition`].
    AmbiguousGenerationLog(String),
    /// The requested object type cannot be persisted in the requested store.
    UnsupportedObjectType(String),
    /// RFC 158 Stage A: an object frame inside an incoming artifact (a bundle or a patch-exchange
    /// artifact) declared a size over the caller's per-object bound. Checked on the length prefix,
    /// before the frame is copied or decoded — the frame's bytes were never read into memory. This
    /// variant's own `Display` deliberately says neither "malformed" nor "persisted": the input may
    /// be perfectly well-formed, just larger than this caller is willing to accept, and the CLI
    /// layer re-renders this into a refusal naming where the bound came from and how to change it
    /// (`prikk-store` has no notion of a flag or a config file to name).
    ObjectOverBound {
        /// The size this frame declared, in bytes.
        declared_bytes: u64,
        /// The per-object bound that was exceeded, in bytes.
        bound_bytes: u64,
    },
    /// RFC 155 Part I-C (external review 023): a DC-86 bound on a *count* -- entries, sections,
    /// records -- not a byte length. [`Self::ObjectOverBound`]'s own `Display` text says "bytes"
    /// unconditionally, which misleads when the thing actually bounded is a count; this variant
    /// exists so a count-bound refusal reads honestly, without borrowing byte-shaped wording for a
    /// unit it never had.
    ///
    /// I-E item 3 (review `rfc155-impl-IC-review-v1`): the first version named no subject at all
    /// (*"declares 20000 entries, over the bound of 10000"* -- a user cannot tell what declared
    /// them). `declared_what`/`unit` are the call site's own words for what was counted, so every
    /// construction site states its own subject explicitly rather than this shared variant
    /// guessing one.
    CountOverBound {
        /// What owns the count, in the call site's own words (e.g. `"the archive manifest"`).
        declared_what: &'static str,
        /// The plural noun for what was counted (e.g. `"sections"`).
        unit: &'static str,
        /// The count this frame declared.
        declared_count: u64,
        /// The bound that was exceeded.
        bound_count: u64,
    },
    /// 0.56.0 step 3b (A4), generalized in step 3c2 (R2): `bundle import`'s and `archive import`'s
    /// own refusal of a rollback, a diverged rewrite, or a history that does not connect to the
    /// pointer it would replace (0.56.0 step 3a, A3). Exists apart from [`Self::Precondition`] for
    /// the same reason [`Self::IncompletePublication`] does: `prikk-cli`'s own `refusal-v1`
    /// document must classify this refusal structurally, never by matching this error's own
    /// message text (an edit to the text could silently drop the classification). Its own
    /// `Display` text is unchanged from what `Precondition` would have rendered for the identical
    /// message; only a caller that needs the class (or which command raised it) matches the
    /// variant.
    ///
    /// **One variant for both importers, not two** (R2's own choice, stated here so a future
    /// reader does not wonder why `archive import` does not get its own sibling): the two call
    /// sites differ only in which command raised the refusal and in how the "prior pointer" side
    /// is resolved (`bundle import`'s own pass-1 metadata vs. `archive import`'s local store /
    /// archive-side graph) -- a difference already fully absorbed before either ever constructs
    /// this value. The `class`/`message` fields, and every caller that matches on them, are
    /// otherwise identical between the two, so a second, structurally-duplicate variant would
    /// carry no information a `command` field does not already carry more cheaply.
    BundleNonFastForward {
        /// Which command raised this refusal: `"bundle-import"` or `"archive-import"` -- matches
        /// `refusal-v1`'s own `"command"` field for each (`bundle-import-report-v1`'s is
        /// `"bundle-import"`; `prepo-import-report-v1`'s new one is `"archive-import"`).
        command: &'static str,
        /// The connectivity class that was refused: one of `"rollback"`, `"diverged"`, or
        /// `"does-not-connect"` (`prikk_store::BundleConnectivityClass::as_str`'s own values for
        /// those three variants). Kept as a plain `&'static str` here, not `prikk-store`'s own
        /// enum: `prikk-error` sits below `prikk-store` in the dependency graph and must not depend
        /// on it.
        class: &'static str,
        /// The fully rendered message, exactly what `Precondition`'s own `Display` would have
        /// carried for this refusal.
        message: String,
    },
    /// An I/O failure. `kind` is `Some` only when this value was built from a real
    /// `std::io::Error` via [`From`] -- every explicit construction site elsewhere in the workspace
    /// (a caller-precondition violation, a platform-capability refusal, or a validation failure
    /// wearing this variant rather than one that describes it) sets `kind: None`, which is the
    /// truth, not a placeholder to "tidy" into something non-optional. RFC 132 increment 2 is
    /// expected to move those sites onto variants that describe them and narrow this field.
    Io {
        /// The underlying `std::io::ErrorKind`, when this was built from a real `std::io::Error`.
        kind: Option<std::io::ErrorKind>,
        /// Human-readable context. Alone carries the full `Display` message -- see that impl.
        context: String,
    },
}

impl fmt::Display for PrikkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CanonicalEncoding(msg) => write!(f, "canonical encoding error: {msg}"),
            Self::InvalidObjectId(msg) => write!(f, "invalid object id: {msg}"),
            Self::InvalidSignature(msg) => write!(f, "invalid signature: {msg}"),
            Self::InvalidName(msg) => write!(f, "invalid name: {msg}"),
            Self::ObjectTypeMismatch { expected, actual } => {
                write!(f, "object type mismatch: expected {expected}, got {actual}")
            }
            Self::UnsupportedFormatVersion(version) => {
                write!(f, "unsupported format version: {version}")
            }
            Self::MalformedData(msg) => write!(f, "malformed persisted data: {msg}"),
            Self::Integrity(msg) => write!(f, "integrity error: {msg}"),
            Self::LockConflict(msg) => write!(f, "lock conflict: {msg}"),
            Self::Precondition(msg) => write!(f, "precondition not met: {msg}"),
            Self::IncompletePublication(msg) => write!(f, "precondition not met: {msg}"),
            Self::BundleNonFastForward { message, .. } => {
                write!(f, "precondition not met: {message}")
            }
            Self::AmbiguousGenerationLog(msg) => write!(f, "ambiguous generation log: {msg}"),
            Self::UnsupportedObjectType(msg) => write!(f, "unsupported object type: {msg}"),
            Self::ObjectOverBound {
                declared_bytes,
                bound_bytes,
            } => write!(
                f,
                "an object declares {declared_bytes} bytes, over the per-object bound of \
                 {bound_bytes} bytes"
            ),
            Self::CountOverBound {
                declared_what,
                unit,
                declared_count,
                bound_count,
            } => write!(
                f,
                "{declared_what} declares {declared_count} {unit}, over the bound of {bound_count}"
            ),
            Self::Io { context, .. } => write!(f, "i/o error: {context}"),
        }
    }
}

/// 0.56.0 step 3d (A6): whether a decode/verification failure is genuine corruption, or specifically
/// this version's own path rule refusing a path that a newer `prikk` version may have written. A
/// patch written by a newer binary decodes fine as a structural shape but can fail this one semantic
/// check (today: [`PrikkError::InvalidName`]'s non-ASCII-path refusal) -- that is not damage, and a
/// caller that needs to say so distinctly (rather than calling a healthy repository "malformed")
/// matches this value, never the rendered `Display` text, which can be edited without anyone
/// noticing the classification silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeFailureKind {
    /// A truncated varint, bad canonical encoding, wrong field count, or other real damage.
    Corrupt,
    /// This version's own path rule refuses a path; a newer `prikk` version may have written it.
    OutsideVersionRule,
}

impl PrikkError {
    /// Classify this error as corruption or a semantic path-rule refusal (0.56.0 step 3d, A6).
    /// Every wrapping site between a `Patch`'s own operations and a user-facing message calls this
    /// *before* folding the error into a `String`, so the classification survives the fold instead
    /// of being re-derived (or lost) later from text that can drift.
    #[must_use]
    pub fn decode_failure_kind(&self) -> DecodeFailureKind {
        match self {
            Self::InvalidName(_) => DecodeFailureKind::OutsideVersionRule,
            _ => DecodeFailureKind::Corrupt,
        }
    }
}

impl std::error::Error for PrikkError {}

impl From<std::io::Error> for PrikkError {
    fn from(value: std::io::Error) -> Self {
        Self::Io {
            kind: Some(value.kind()),
            context: value.to_string(),
        }
    }
}
