//! Store-backed lifecycle resolvers (DC-09 Phase 4.4-2c-1).
//!
//! Bridges the lifecycle-cache resolver traits to the object store, generic over
//! [`ObjectReader`] so it works with the file store or an in-memory store. Closes
//! carry-forward P2-1: a **missing or unreadable block is an error, never genesis** — only a
//! successfully-decoded `Block` with zero parents is genesis. A missing blob returns the
//! fail-closed `Ok(None)` sentinel; a present-but-wrong-type object is an error.
//!
//! This increment wires no replay and makes no identity decision.

use prikk_error::{PrikkError, Result};
#[cfg(test)]
use prikk_object::BlockPayload;
use prikk_object::{BlobKind, BlobPayload, ObjectId, ObjectType};

#[cfg(test)]
use super::BlockParentResolver;
use super::{BlobContentResolver, BlobKindResolver};
use crate::object_store::ObjectReader;
/// Lifecycle resolver backed by any object reader (file or memory store).
pub(in crate::lifecycle_cache) struct StoreBackedResolver<'a, R: ObjectReader> {
    reader: &'a R,
    require_schema_one: bool,
}

impl<'a, R: ObjectReader> StoreBackedResolver<'a, R> {
    pub(crate) fn new(reader: &'a R) -> Self {
        Self {
            reader,
            require_schema_one: false,
        }
    }

    pub(crate) fn new_format2(reader: &'a R) -> Self {
        Self {
            reader,
            require_schema_one: true,
        }
    }

    fn validate_blob_schema(&self, schema_version: u32) -> Result<()> {
        // Patch schema 2 handoff v2 amendment §2: read the same authoritative admitted-schema
        // table `format.rs::validate_format2_schema` owns, rather than a second, hand-maintained
        // `!= 1` check. `Blob` is admitted at `&[1]` only (unaffected by the Patch schema this
        // amendment introduces), so this generalization leaves `Blob`'s own behaviour
        // byte-identical while making it correct-by-construction against the one real table.
        if self.require_schema_one {
            let accepted = crate::format::admitted_schemas(ObjectType::Blob).unwrap_or(&[]);
            if !accepted.contains(&schema_version) {
                return Err(PrikkError::Integrity(format!(
                    "format-2 Blob does not accept envelope schema {schema_version} (accepted: {accepted:?})"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
impl<R: ObjectReader> BlockParentResolver for StoreBackedResolver<'_, R> {
    fn parent_block_ids(&self, block_id: &ObjectId) -> Result<Vec<ObjectId>> {
        let Some(envelope) = self.reader.read_object(*block_id)? else {
            // P2-1: a missing block must not be mistaken for genesis.
            return Err(PrikkError::Integrity(format!(
                "lifecycle replay: block {block_id} is missing and cannot be treated as genesis"
            )));
        };
        if envelope.object_type != ObjectType::Block {
            return Err(PrikkError::Integrity(format!(
                "lifecycle replay: object {block_id} is not a Block ({} found)",
                envelope.object_type
            )));
        }
        let block = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        Ok(block.parent_block_ids)
    }
}

impl<R: ObjectReader> BlobKindResolver for StoreBackedResolver<'_, R> {
    fn blob_kind(&self, blob_id: &ObjectId) -> Result<Option<BlobKind>> {
        let Some(envelope) = self.reader.read_object(*blob_id)? else {
            // Absent blob: fail-closed sentinel — the cache entry is unusable, not "fresh".
            return Ok(None);
        };
        if envelope.object_type != ObjectType::Blob {
            return Err(PrikkError::Integrity(format!(
                "lifecycle replay: object {blob_id} is not a Blob ({} found)",
                envelope.object_type
            )));
        }
        self.validate_blob_schema(envelope.schema_version)?;
        let blob = BlobPayload::decode_canonical(&envelope.canonical_payload)?;
        Ok(Some(blob.blob_kind))
    }
}

impl<R: ObjectReader> BlobContentResolver for StoreBackedResolver<'_, R> {
    fn blob_content(&self, blob_id: &ObjectId) -> Result<Option<(BlobKind, Vec<u8>)>> {
        let Some(envelope) = self.reader.read_object(*blob_id)? else {
            return Ok(None);
        };
        if envelope.object_type != ObjectType::Blob {
            return Err(PrikkError::Integrity(format!(
                "lifecycle replay: object {blob_id} is not a Blob ({} found)",
                envelope.object_type
            )));
        }
        self.validate_blob_schema(envelope.schema_version)?;
        let blob = BlobPayload::decode_canonical(&envelope.canonical_payload)?;
        Ok(Some((blob.blob_kind, blob.content)))
    }
}

#[cfg(test)]
mod tests;
