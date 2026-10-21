use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::super::path::repository_file;
use crate::error::{Error, Result};

pub(super) fn verify_file<T>(root: &Path, identity: &T) -> Result<Vec<u8>>
where
    T: Identity,
{
    let bytes = fs::read(repository_file(root, identity.path())?)?;
    verify_bytes(
        identity.path(),
        &bytes,
        identity.byte_length(),
        identity.sha256(),
    )?;
    Ok(bytes)
}

pub(super) fn verify_bytes(label: &str, bytes: &[u8], length: u64, digest: &str) -> Result<()> {
    if bytes.len() as u64 != length {
        return Err(Error::new(format!("input-identity:length:{label}")));
    }
    // sha2 0.11's `Digest::digest` returns `Array<u8, _>`, which (unlike 0.10's generic-array
    // type) does not implement `LowerHex` -- hex-encoded byte-by-byte instead, the same pattern
    // `release_evidence.rs`'s own `hex_sha256` already uses.
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != digest {
        return Err(Error::new(format!("input-identity:sha256:{label}")));
    }
    Ok(())
}

pub(super) trait Identity {
    fn path(&self) -> &str;
    fn byte_length(&self) -> u64;
    fn sha256(&self) -> &str;
}

impl Identity for super::super::model::FileIdentity {
    fn path(&self) -> &str {
        &self.path
    }

    fn byte_length(&self) -> u64 {
        self.byte_length
    }

    fn sha256(&self) -> &str {
        &self.sha256
    }
}

impl Identity for super::super::model::PackIdentity {
    fn path(&self) -> &str {
        &self.path
    }

    fn byte_length(&self) -> u64 {
        self.byte_length
    }

    fn sha256(&self) -> &str {
        &self.sha256
    }
}
