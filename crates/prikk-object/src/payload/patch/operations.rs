//! Per-operation-kind patch payload structs (FDD-03 §9.3): validation and canonical encoding.
//! Split out of `payload/patch.rs` (DC-58) — no behaviour change, all items moved verbatim.
//! Re-exported at `payload/patch.rs` so every existing public path (`payload::patch::CreateFile`
//! etc., and the crate-root re-export at `payload.rs`) is unchanged.

use prikk_error::{PrikkError, Result};

use crate::path::validate_repo_path;
use crate::payload::node::{NodeId, NodeKind};
use crate::{CanonicalEncode, CanonicalWriter, ObjectId};

use super::{TEXT_SPAN_ANCHOR_MIN_LEN, TEXT_SPAN_HASH_BYTES, text_span_hash};

/// The only two mode values prikk ever authors for a file node (`prikk-store`'s
/// `worktree_patch/node_authoring.rs::REGULAR_FILE_MODE`/`EXECUTABLE_FILE_MODE`, and
/// `state_root.rs`'s own `REGULAR_MODE`/`EXECUTABLE_MODE` seal-time matcher -- all three must keep
/// agreeing on these two values). RFC 125 §2: any other `u32` a decoder would accept -- including
/// setuid/setgid/sticky bits `& 0o7777` admits at materialization
/// (`fsutil/anchored/linux.rs::fchmod`) -- is a mode the encoder never wrote, so refusing it here
/// closes the gap symmetrically rather than leaving decode more permissive than encode.
const CANONICAL_FILE_MODES: [u32; 2] = [0o100_644, 0o100_755];

/// Whether `mode` is one of the two values prikk ever authors for a file node. Exposed so
/// `prikk-store`'s patch-replay decoder can refuse the same non-canonical modes at decode that
/// this module's own `validate()` methods refuse at encode (RFC 125 §2, DC-54 symmetry).
#[must_use]
pub fn is_canonical_file_mode(mode: u32) -> bool {
    CANONICAL_FILE_MODES.contains(&mode)
}

/// Reword a `validate_repo_path` refusal for the authoring context: `commit` encoding a worktree
/// file into a new `Patch` (0.56.0 step 3d2). Structural: matches
/// [`PrikkError::NonAsciiRepoPath`], never any error's rendered text. Every other
/// `validate_repo_path` refusal (empty, absolute, too-long, a reserved component, ...) passes
/// through unchanged -- those are permanently invalid, not a statement about this version
/// specifically, so they keep `InvalidName`'s own wording.
fn authoring_path_error(err: PrikkError) -> PrikkError {
    match err {
        PrikkError::NonAsciiRepoPath { path } => PrikkError::InvalidName(format!(
            "this version of prikk does not accept non-ASCII repository paths: {path}"
        )),
        other => other,
    }
}

/// Create file payload (FDD-03 §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFile {
    /// Repo-relative UTF-8 path (`repo_path`).
    pub path: String,
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Initial blob ID (`object_id`).
    pub blob_id: ObjectId,
    /// Mode bits (`u32`).
    pub mode: u32,
}

impl CreateFile {
    /// Reject an all-zero `node_id`; FDD-03 §9.3 forbids the reserved value in any
    /// persisted node-bearing operation, and the encoder produces identity bytes. Also reject a
    /// `path` that violates the `RepoPath` grammar decode enforces (DC-54): encode must reject
    /// exactly what decode rejects, using the same validator, so the two sides cannot drift.
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "CreateFile node_id must be nonzero".to_string(),
            ));
        }
        validate_repo_path(&self.path).map_err(authoring_path_error)?;
        if !is_canonical_file_mode(self.mode) {
            return Err(PrikkError::CanonicalEncoding(format!(
                "CreateFile mode {:#o} is not one of prikk's canonical file modes",
                self.mode
            )));
        }
        Ok(())
    }
}

impl CanonicalEncode for CreateFile {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_repo_path(1, &self.path)?;
        writer.field_bytes(2, self.node_id.as_bytes())?;
        writer.field_object_id(3, &self.blob_id)?;
        writer.field_u32(4, self.mode)?;
        Ok(())
    }
}

/// Discriminated deletion preimage (FDD-03 §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteNodePreimage {
    /// File or binary node: blob + mode preimage.
    File {
        /// Previous blob ID (`object_id`).
        old_blob_id: ObjectId,
        /// Previous mode bits (`u32`).
        old_mode: u32,
    },
    /// Symlink node: target preimage.
    Symlink {
        /// Previous symlink target (`utf8`).
        old_target: String,
    },
}

/// Delete a node (FDD-03 §9.3; the wire tag is retained as `delete_file`). The
/// preimage is discriminated by `old_node_kind`: text/binary file nodes carry
/// `old_blob_id` + `old_mode`; symlink nodes carry `old_target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteNode {
    /// Repo-relative UTF-8 path (`repo_path`).
    pub path: String,
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Previous node kind (`enum_u16`); must agree with the preimage.
    pub old_node_kind: NodeKind,
    /// Discriminated deletion preimage.
    pub preimage: DeleteNodePreimage,
}

impl DeleteNode {
    /// Reject `old_node_kind` / preimage discriminator mismatches, an all-zero
    /// `node_id` (FDD-03 §9.3 forbids the reserved value in any node-bearing op), and a `path`
    /// that violates the `RepoPath` grammar decode enforces (DC-54).
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "DeleteNode node_id must be nonzero".to_string(),
            ));
        }
        validate_repo_path(&self.path).map_err(authoring_path_error)?;
        let consistent = matches!(
            (self.old_node_kind, &self.preimage),
            (
                NodeKind::TextFile | NodeKind::BinaryFile,
                DeleteNodePreimage::File { .. }
            ) | (NodeKind::Symlink, DeleteNodePreimage::Symlink { .. })
        );
        if !consistent {
            return Err(PrikkError::CanonicalEncoding(
                "DeleteNode old_node_kind does not match preimage discriminator".to_string(),
            ));
        }
        if let DeleteNodePreimage::File { old_mode, .. } = &self.preimage {
            if !is_canonical_file_mode(*old_mode) {
                return Err(PrikkError::CanonicalEncoding(format!(
                    "DeleteNode old_mode {old_mode:#o} is not one of prikk's canonical file modes"
                )));
            }
        }
        Ok(())
    }
}

impl CanonicalEncode for DeleteNode {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_repo_path(1, &self.path)?;
        writer.field_bytes(2, self.node_id.as_bytes())?;
        writer.field_enum_u16(3, self.old_node_kind.code())?;
        match &self.preimage {
            DeleteNodePreimage::File {
                old_blob_id,
                old_mode,
            } => {
                writer.field_object_id(4, old_blob_id)?;
                writer.field_u32(6, *old_mode)?;
            }
            DeleteNodePreimage::Symlink { old_target } => {
                writer.field_string(5, old_target)?;
            }
        }
        Ok(())
    }
}

/// Text edit payload using content-anchor identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditText {
    /// Node identity (`bytes`, 32). EditText is node-addressed, not path-addressed.
    pub node_id: NodeId,
    /// Content-anchor span identity (`bytes`, 32; FDD-01 §5.1).
    pub span_id: [u8; TEXT_SPAN_HASH_BYTES],
    /// SHA-256 of `old_span_text`; the validator binds the two.
    pub old_span_hash: [u8; TEXT_SPAN_HASH_BYTES],
    /// Bounded left-context hash (`bytes`, 32).
    pub left_anchor_hash: [u8; TEXT_SPAN_HASH_BYTES],
    /// Bounded right-context hash (`bytes`, 32).
    pub right_anchor_hash: [u8; TEXT_SPAN_HASH_BYTES],
    /// New span bytes (`bytes`); UTF-8 text for v1, stored verbatim (never NFC).
    pub replacement_text: Vec<u8>,
    /// Optional presentation hint (line); not part of algebraic identity.
    pub presentation_hint_line: Option<u32>,
    /// Optional presentation hint (column); not part of algebraic identity.
    pub presentation_hint_column: Option<u32>,
    /// Old span bytes (`bytes`); UTF-8 for v1, verbatim; inverse material.
    pub old_span_text: Vec<u8>,
    /// RFC 134 §8: v2 content-unique identity's recorded left-anchor length, in bytes. Present
    /// only at schema `PATCH_TEXT_SPAN_V2_SCHEMA` and above, and only together with
    /// `right_anchor_len` — never alone. Absent (`None`) means `span_id` is v1's positional
    /// identity; present means it is v2's content-unique identity computed over anchors of
    /// exactly this length.
    pub left_anchor_len: Option<u32>,
    /// RFC 134 §8: v2 content-unique identity's recorded right-anchor length, in bytes. See
    /// `left_anchor_len` — always present or absent together with it.
    pub right_anchor_len: Option<u32>,
}

impl EditText {
    /// Validate the FDD-03 §9.3 EditText record contract: nonzero `node_id`,
    /// `old_span_hash == SHA-256(old_span_text)`, both span-text fields are well-formed UTF-8
    /// (non-UTF-8 content must use `ReplaceBinary`), and — RFC 134 §8 — `left_anchor_len` and
    /// `right_anchor_len` are both present or both absent, each at least
    /// `TEXT_SPAN_ANCHOR_MIN_LEN` when present.
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "EditText node_id must be nonzero".to_string(),
            ));
        }
        if self.old_span_hash != text_span_hash(&self.old_span_text) {
            return Err(PrikkError::CanonicalEncoding(
                "EditText old_span_hash must equal SHA-256(old_span_text)".to_string(),
            ));
        }
        if core::str::from_utf8(&self.old_span_text).is_err() {
            return Err(PrikkError::CanonicalEncoding(
                "EditText old_span_text must be well-formed UTF-8".to_string(),
            ));
        }
        if core::str::from_utf8(&self.replacement_text).is_err() {
            return Err(PrikkError::CanonicalEncoding(
                "EditText replacement_text must be well-formed UTF-8".to_string(),
            ));
        }
        match (self.left_anchor_len, self.right_anchor_len) {
            (None, None) => {}
            (Some(left), Some(right)) => {
                if left < TEXT_SPAN_ANCHOR_MIN_LEN || right < TEXT_SPAN_ANCHOR_MIN_LEN {
                    return Err(PrikkError::CanonicalEncoding(format!(
                        "EditText anchor lengths must each be at least {TEXT_SPAN_ANCHOR_MIN_LEN}"
                    )));
                }
            }
            _ => {
                return Err(PrikkError::CanonicalEncoding(
                    "EditText left_anchor_len and right_anchor_len must be both present or both absent"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

impl CanonicalEncode for EditText {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_bytes(1, self.node_id.as_bytes())?;
        writer.field_bytes(2, &self.span_id)?;
        writer.field_bytes(3, &self.old_span_hash)?;
        writer.field_bytes(4, &self.left_anchor_hash)?;
        writer.field_bytes(5, &self.right_anchor_hash)?;
        writer.field_bytes(6, &self.replacement_text)?;
        if let Some(line) = self.presentation_hint_line {
            writer.field_u32(7, line)?;
        }
        if let Some(column) = self.presentation_hint_column {
            writer.field_u32(8, column)?;
        }
        writer.field_bytes(9, &self.old_span_text)?;
        if let Some(left) = self.left_anchor_len {
            writer.field_u32(10, left)?;
        }
        if let Some(right) = self.right_anchor_len {
            writer.field_u32(11, right)?;
        }
        Ok(())
    }
}

/// Rename path payload (FDD-03 §9.3, node-addressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePath {
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Old repo-relative path (`repo_path`).
    pub old_path: String,
    /// New repo-relative path (`repo_path`).
    pub new_path: String,
}

impl RenamePath {
    /// Reject an all-zero `node_id`; FDD-03 §9.3 forbids the reserved value in any
    /// persisted node-bearing operation, and the encoder produces identity bytes. Also reject an
    /// `old_path` or `new_path` that violates the `RepoPath` grammar decode enforces (DC-54) —
    /// both fields are checked independently, since a path-safe `new_path` must not mask an
    /// unsafe `old_path`.
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "RenamePath node_id must be nonzero".to_string(),
            ));
        }
        validate_repo_path(&self.old_path).map_err(authoring_path_error)?;
        validate_repo_path(&self.new_path).map_err(authoring_path_error)?;
        Ok(())
    }
}

impl CanonicalEncode for RenamePath {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_bytes(1, self.node_id.as_bytes())?;
        writer.field_repo_path(2, &self.old_path)?;
        writer.field_repo_path(3, &self.new_path)?;
        Ok(())
    }
}

/// Permission change payload (FDD-03 §9.3, node-addressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePerm {
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Old mode bits (`u32`).
    pub old_mode: u32,
    /// New mode bits (`u32`).
    pub new_mode: u32,
}

impl ChangePerm {
    /// Reject an all-zero `node_id` (FDD-03 §9.3).
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "ChangePerm node_id must be nonzero".to_string(),
            ));
        }
        if !is_canonical_file_mode(self.old_mode) {
            return Err(PrikkError::CanonicalEncoding(format!(
                "ChangePerm old_mode {:#o} is not one of prikk's canonical file modes",
                self.old_mode
            )));
        }
        if !is_canonical_file_mode(self.new_mode) {
            return Err(PrikkError::CanonicalEncoding(format!(
                "ChangePerm new_mode {:#o} is not one of prikk's canonical file modes",
                self.new_mode
            )));
        }
        Ok(())
    }
}

impl CanonicalEncode for ChangePerm {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_bytes(1, self.node_id.as_bytes())?;
        writer.field_u32(2, self.old_mode)?;
        writer.field_u32(3, self.new_mode)?;
        Ok(())
    }
}

/// Symlink creation payload (FDD-03 §9.3). Note tag order: `path` (1), then
/// `node_id` (2), then `target` (3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSymlink {
    /// Repo-relative UTF-8 path (`repo_path`).
    pub path: String,
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Symlink target (`utf8_string`). Static escape/four-boundary validation
    /// (FDD-04 §5.4a / §13.1) is a later increment; this reconciles identity bytes.
    pub target: String,
}

impl CreateSymlink {
    /// Reject an all-zero `node_id` (FDD-03 §9.3) and a `path` that violates the `RepoPath`
    /// grammar decode enforces (DC-54). `target` is deliberately left untouched: it is an opaque
    /// symlink target by accepted DC-40 design, not a repository-relative path.
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "CreateSymlink node_id must be nonzero".to_string(),
            ));
        }
        validate_repo_path(&self.path).map_err(authoring_path_error)?;
        Ok(())
    }
}

impl CanonicalEncode for CreateSymlink {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_repo_path(1, &self.path)?;
        writer.field_bytes(2, self.node_id.as_bytes())?;
        writer.field_string(3, &self.target)?;
        Ok(())
    }
}

/// Binary replacement payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceBinary {
    /// Node identity (`bytes`, 32).
    pub node_id: NodeId,
    /// Old blob ID (`object_id`).
    pub old_blob_id: ObjectId,
    /// New blob ID (`object_id`).
    pub new_blob_id: ObjectId,
}

impl ReplaceBinary {
    /// Reject an all-zero `node_id`; FDD-03 §9.3 forbids the reserved value in any
    /// persisted node-bearing operation, and the encoder produces identity bytes.
    pub fn validate(&self) -> Result<()> {
        if self.node_id.is_zero() {
            return Err(PrikkError::CanonicalEncoding(
                "ReplaceBinary node_id must be nonzero".to_string(),
            ));
        }
        Ok(())
    }
}

impl CanonicalEncode for ReplaceBinary {
    fn encode_canonical(&self, writer: &mut CanonicalWriter) -> Result<()> {
        self.validate()?;
        writer.field_bytes(1, self.node_id.as_bytes())?;
        writer.field_object_id(2, &self.old_blob_id)?;
        writer.field_object_id(3, &self.new_blob_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.56.0 step 3d2: `CreateFile::validate`'s own `validate_repo_path` call must give the
    /// *authoring* sentence for a non-ASCII path -- naming the file, never the *reading* sentence
    /// (which names "newer version of prikk may have written it", wrong for a worktree file that
    /// was just typed by hand) and never the retired "deferred until ... implemented" wording.
    #[test]
    #[allow(clippy::expect_used)]
    fn create_file_validate_gives_the_authoring_sentence_for_a_non_ascii_path() {
        let op = CreateFile {
            path: "pages/日本語.md".to_string(),
            node_id: NodeId::from_bytes([1u8; 32]),
            blob_id: ObjectId::from_bytes([2u8; 32]),
            mode: 0o100_644,
        };
        let err = op
            .validate()
            .expect_err("a non-ASCII path must still refuse authoring");
        let rendered = err.to_string();
        assert!(
            rendered.contains("this version of prikk does not accept non-ASCII repository paths")
                && rendered.contains("pages/日本語.md"),
            "expected the authoring sentence naming the file, got: {rendered}"
        );
        assert!(
            !rendered.contains("deferred") && !rendered.contains("newer version of prikk may have"),
            "authoring must never give the reading sentence or the retired wording: {rendered}"
        );
    }
}
