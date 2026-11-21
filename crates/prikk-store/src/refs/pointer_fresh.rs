//! 0.59.0 RS2b: *an id that a pointer named is declared missing only after a lookup at least as new as the pointer read.*
//!
//! A publication writes the objects a ref's tip needs (the Block, the RefState), then the pointer, then the log record. An object handle a command
//! opened **before** it read the pointer (`ObjectReadSnapshot` and `ObjectWriteSession` are a point in time and never refresh on a read) can therefore
//! lack an object the pointer names, for a healthy repository. [`PointerFreshReader`] wraps such a handle: a **hit costs nothing** (the handle answers,
//! and the rest of a reader's snapshot keeps its point in time); a **miss** is looked up once more through a `FileObjectStore`, which checks the object
//! index on every lookup, and only a miss that survives that is a missing object -- reported by the caller's own message, as before.
//!
//! The index is append-only, so a newer view cannot change the answer for an id the handle already holds; and it takes no lock.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, RefStatePayload};

use super::{RefStore, resolve_ref_tip_block};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{FileObjectStore, ObjectReadSnapshot, ObjectReader, ObjectWriteSession};

/// An object handle plus the one more lookup a miss gets. See the module doc.
pub(crate) struct PointerFreshReader<'a, R: ObjectReader + ?Sized> {
    inner: &'a R,
    live: FileObjectStore,
}

impl<'a, R: ObjectReader + ?Sized> PointerFreshReader<'a, R> {
    /// Wrap `inner`, a handle that may have been opened before the pointer it is used with was read.
    pub(crate) fn over(layout: &RepositoryLayout, inner: &'a R) -> Self {
        Self {
            inner,
            live: FileObjectStore::new(layout.clone()),
        }
    }
}

impl<R: ObjectReader + ?Sized> ObjectReader for PointerFreshReader<'_, R> {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        match self.inner.read_object(id)? {
            Some(envelope) => Ok(Some(envelope)),
            None => self.live.read_object(id),
        }
    }

    fn has_object(&self, id: ObjectId, object_type: ObjectType) -> Result<bool> {
        if self.inner.has_object(id, object_type)? {
            return Ok(true);
        }
        self.live.has_object(id, object_type)
    }
}

/// A handle that can be brought up to date in place: [`ObjectReadSnapshot`] and [`ObjectWriteSession`], the two a command opens and keeps.
pub(crate) trait Refreshable: ObjectReader {
    /// Bring the handle up to the object index as it is now (a stat, the last record, and a decode of what was appended).
    fn refresh(&mut self) -> Result<()>;
}

impl Refreshable for ObjectReadSnapshot {
    fn refresh(&mut self) -> Result<()> {
        ObjectReadSnapshot::refresh(self)
    }
}

impl Refreshable for ObjectWriteSession {
    fn refresh(&mut self) -> Result<()> {
        ObjectWriteSession::refresh(self)
    }
}

/// The object a pointer named, for a caller that keeps a **mutable** handle opened before it read the pointer: a hit is free; a miss brings the
/// handle up to date once and looks again. Everything the object needs was durable before it, so a handle that holds it holds its Blocks and
/// ancestors too -- the caller's later reads from the same handle do not miss. `None` only if the object is missing from the index as it is now.
pub(crate) fn read_pointer_named<R: Refreshable>(
    reader: &mut R,
    id: ObjectId,
    object_type: ObjectType,
) -> Result<Option<ObjectEnvelope>> {
    if let Some(envelope) = reader.read_typed(id, object_type)? {
        return Ok(Some(envelope));
    }
    reader.refresh()?;
    reader.read_typed(id, object_type)
}

/// RFC 147 §3d: `patch_replay/read.rs` and `patch_inverse/read.rs` each carried a `pub(super) fn
/// current_target_block` with the same signature and a byte-identical body. That duplication is
/// exactly how §3b's fix reached one and not the other, leaving `inverse-plan`, `rollback-preview`
/// and `rollback-draft-verify` refusing a valid tag ref while `log` and `checkout` accepted it --
/// and a comment in one of them asserted, wrongly, that they were shared. One function now, here,
/// because both callers already depend on this module and neither depends on the other: putting it
/// in either of them would invent a lateral `patch_replay <-> patch_inverse` edge for nothing.
///
/// This is [`resolve_ref_tip_block`] plus the read that gets you a [`RefStatePayload`] in the first
/// place; that function stays separate because several callers already hold a decoded payload and
/// must not re-read it. Like it, this **resolves and never validates** -- the returned Block id is
/// not checked to exist, which is each caller's own job (see `ensure_ref_target_valid`).
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn read_current_ref_tip_block(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
) -> Result<ObjectId> {
    let ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id(ref_name)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} is not published")))?;
    ref_tip_block_of(layout, object_store, ref_name, ref_state_id)
}

/// The Block `ref_state_id` (the pointer's RefState for `ref_name`, already read) resolves to. 0.59.0 RS2b: the RefState, and the Tag a tag ref
/// names, are looked up through [`PointerFreshReader`], so a handle opened before the pointer was read cannot call them missing.
pub(crate) fn ref_tip_block_of(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    ref_state_id: ObjectId,
) -> Result<ObjectId> {
    let object_store = &PointerFreshReader::over(layout, object_store);
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} points to missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if ref_state.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState name mismatch: expected {ref_name}, got {}",
            ref_state.ref_name
        )));
    }
    let (block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state)?;
    Ok(block_id)
}

/// The tip Block of a ref, for a caller that has no object handle yet (0.59.0 RS2b): the pointer is read **first** and the snapshot is opened
/// **after** it, so the snapshot holds every object the pointer names -- the RefState, its Block and the Block's ancestors were durable before the
/// pointer was written. Returns the snapshot with the tip, for the caller's walk.
pub(crate) fn open_at_ref_tip(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(ObjectReadSnapshot, ObjectId)> {
    let ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id(ref_name)?
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} is not published")))?;
    #[cfg(test)]
    super::view_hooks::run("tip-between-pointer-and-snapshot");
    let objects = ObjectReadSnapshot::open(layout)?;
    let tip = ref_tip_block_of(layout, &objects, ref_name, ref_state_id)?;
    Ok((objects, tip))
}
