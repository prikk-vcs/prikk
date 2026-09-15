//! The replay-verified block record (RFC 136 increment 2b, §10.3c ruling 2).
//!
//! A monotonic set of Block ids whose `state_merkle_root` this repository confirmed by replay. State
//! read from a snapshot may reach the worktree only through an anchor in this set; any other anchored
//! worktree write replays from genesis.
//!
//! **Rebuildable and never authoritative**, like the DC-64 lifecycle cache. A missing, truncated,
//! mis-checksummed or unreadable file, or one written by any other `prikk-store` version, reads as the
//! empty set, never an error: replay then runs in full. Keying the file to the crate version means a
//! replay fix never inherits a verdict an older replay reached.
//!
//! **Writers** are only the operations that have just confirmed roots by replay: `seal_block` (the
//! parent lineage its derivation verified, plus the new Block, whose root it computed) and `verify`
//! (every Block whose state outcome is `Verified`). Nothing that merely receives or points at blocks
//! writes here.
//!
//! **Concurrency.** Each write is a read-merge-write through an atomic replace. Two writers racing can
//! each read before the other writes; the later replace then drops ids only the earlier one added. That
//! loses entries -- the safe direction -- and never adds one: every id in either writer's set was either
//! read from the record or verified by that writer.

use std::collections::BTreeSet;
use std::path::PathBuf;

use prikk_object::ObjectId;

use crate::foundation::fsutil::{read_file_if_exists, write_file_atomically};
use crate::foundation::layout::RepositoryLayout;

const RECORD_FILE_NAME: &str = "replay-verified-blocks.v1";
const RECORD_MAGIC: &[u8] = b"PRIKK-REPLAY-VERIFIED-BLOCKS-v1\0";
const RECORD_SCHEMA_VERSION: u32 = 1;
/// The replay semantics a recorded verdict was reached under.
const REPLAY_SEMANTICS_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn record_path(layout: &RepositoryLayout) -> PathBuf {
    layout.cache_dir().join(RECORD_FILE_NAME)
}

/// The recorded set, or the empty set on any failure.
pub(crate) fn load_verified_blocks(layout: &RepositoryLayout) -> BTreeSet<ObjectId> {
    load(layout).unwrap_or_default()
}

fn load(layout: &RepositoryLayout) -> Option<BTreeSet<ObjectId>> {
    let relative = layout.repository_relative(&record_path(layout)).ok()?;
    let bytes = read_file_if_exists(layout.repository_mutation_root(), &relative).ok()??;
    decode(&bytes)
}

/// Add `ids`, each of which the caller has just confirmed by replay. Best-effort: a failure to write
/// leaves fewer entries, never an error.
pub(crate) fn record_verified_blocks(
    layout: &RepositoryLayout,
    ids: impl IntoIterator<Item = ObjectId>,
) {
    let mut set = load_verified_blocks(layout);
    let before = set.len();
    set.extend(ids);
    if set.len() == before {
        return;
    }
    let Ok(relative) = layout.repository_relative(&record_path(layout)) else {
        return;
    };
    let _ = write_file_atomically(layout.repository_mutation_root(), &relative, &encode(&set));
}

fn encode(set: &BTreeSet<ObjectId>) -> Vec<u8> {
    let version = REPLAY_SEMANTICS_VERSION.as_bytes();
    let mut body = Vec::with_capacity(4 + 2 + version.len() + 4 + 32 * set.len());
    body.extend_from_slice(&RECORD_SCHEMA_VERSION.to_be_bytes());
    body.extend_from_slice(
        &u16::try_from(version.len())
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    body.extend_from_slice(version);
    body.extend_from_slice(&u32::try_from(set.len()).unwrap_or(u32::MAX).to_be_bytes());
    for id in set {
        body.extend_from_slice(id.as_bytes());
    }
    let mut out = Vec::with_capacity(RECORD_MAGIC.len() + 32 + body.len());
    out.extend_from_slice(RECORD_MAGIC);
    out.extend_from_slice(&prikk_hash::sha256(&body));
    out.extend_from_slice(&body);
    out
}

fn decode(bytes: &[u8]) -> Option<BTreeSet<ObjectId>> {
    let after_magic = bytes.strip_prefix(RECORD_MAGIC)?;
    let (checksum, body) = after_magic.split_at_checked(32)?;
    if prikk_hash::sha256(body) != checksum {
        return None;
    }
    let (schema, rest) = body.split_at_checked(4)?;
    if u32::from_be_bytes(schema.try_into().ok()?) != RECORD_SCHEMA_VERSION {
        return None;
    }
    let (version_len, rest) = rest.split_at_checked(2)?;
    let version_len = usize::from(u16::from_be_bytes(version_len.try_into().ok()?));
    let (version, rest) = rest.split_at_checked(version_len)?;
    if version != REPLAY_SEMANTICS_VERSION.as_bytes() {
        return None;
    }
    let (count, rest) = rest.split_at_checked(4)?;
    let count = usize::try_from(u32::from_be_bytes(count.try_into().ok()?)).ok()?;
    if rest.len() != count.checked_mul(32)? {
        return None;
    }
    rest.chunks_exact(32)
        .map(|chunk| chunk.try_into().ok().map(ObjectId::from_bytes))
        .collect()
}

#[cfg(test)]
mod tests;
