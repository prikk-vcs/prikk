//! AUD-08: `seal` refuses rather than wraps when the next `update_seq` would overflow `u64`.
//!
//! A `RefStatePayload.update_seq` decodes straight off the wire (`field.read_u64()?`, no upper-bound
//! check), so a repository can hold a `heads/main` whose current RefState decodes with
//! `update_seq == u64::MAX` -- not only through years of ordinary use, but through any RefState
//! object that decodes to that value, however it got there. This test builds exactly such a RefState
//! directly (the same shape `seal` itself would have written, with `update_seq` forced to
//! `u64::MAX`), points `heads/main` at it via the existing
//! `force_ref_pointer_to_arbitrary_state_for_test_support` test hook (already used by
//! `seal_recovery.rs` for the same kind of "repoint at a real, validly-shaped object" control), and
//! confirms `seal` refuses with the new site-specific message rather than silently wrapping back to
//! `0`.
//!
//! The second commit that gives the active WAL something new to seal runs *before* the pointer is
//! corrupted, not after: `seal_active_no_audit` itself never calls
//! `refs::ensure_no_incomplete_publication` (confirmed by inspection), but ordinary `commit` does
//! (via `node_authoring.rs`/`active.rs::append_patch`), and a corrupted-but-uncommitted-to pointer
//! trips that unrelated global consistency gate before this test ever reaches the line under test.
//! Committing first and corrupting the pointer only afterward avoids exercising a different defect
//! than the one this test is for.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

use prikk_object::{CanonicalEncode, ObjectEnvelope, ObjectType, RefStatePayload};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, ObjectWriter,
    RefStore, RepositoryLayout, maintainer_signature,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const AUTHOR_SEED: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const MAINTAINER_SEED: &str = "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000";

fn prikk(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prikk"));
    command.current_dir(root);
    command
}

fn require_success(output: &Output, action: &str) -> TestResult {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn unique_root(tag: &str) -> TestResult<PathBuf> {
    let path = std::env::temp_dir().join(format!("prikk-aud08-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn hex_seed(value: &str) -> TestResult<[u8; 32]> {
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(bytes)
}

#[test]
fn seal_refuses_rather_than_wraps_when_update_seq_would_overflow() -> TestResult {
    let root = unique_root("seal")?;
    require_success(&prikk(&root).arg("init").output()?, "init")?;
    std::fs::write(root.join("state.txt"), b"one\n")?;
    require_success(
        &prikk(&root)
            .env("PRIKK_AUTHOR_KEY_ID", "aud08-author")
            .env("PRIKK_AUTHOR_SEED_FILE", support::seed_file(AUTHOR_SEED))
            .args(["commit", "-m", "one"])
            .output()?,
        "genesis commit",
    )?;
    let signer =
        Ed25519MaintainerSigner::from_seed("aud08-maintainer", &hex_seed(MAINTAINER_SEED)?)?;
    let public_key: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    require_success(
        &prikk(&root)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "aud08-maintainer",
                "--public-key",
                &public_key,
            ])
            .output()?,
        "trust maintainer add",
    )?;
    require_success(
        &prikk(&root)
            .env("PRIKK_MAINTAINER_KEY_ID", "aud08-maintainer")
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(MAINTAINER_SEED),
            )
            .args(["seal", "--allow-no-audit"])
            .output()?,
        "genesis seal",
    )?;

    // A real content change, committed while the repository is still fully consistent -- so the
    // active WAL's patch ids differ from the genesis block's own, and the pointer corruption below
    // (introduced only after this commit) is never seen by `commit`'s own
    // `ensure_no_incomplete_publication` check.
    std::fs::write(root.join("state.txt"), b"two\n")?;
    require_success(
        &prikk(&root)
            .env("PRIKK_AUTHOR_KEY_ID", "aud08-author")
            .env("PRIKK_AUTHOR_SEED_FILE", support::seed_file(AUTHOR_SEED))
            .args(["commit", "-m", "two"])
            .output()?,
        "second commit",
    )?;

    let layout = RepositoryLayout::open(root.clone())?;
    let ref_store = RefStore::new(layout.clone());
    let genesis_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")?
        .ok_or("expected heads/main to already be published")?;
    let mut object_store = FileObjectStore::new(layout.clone());
    let genesis_envelope = object_store
        .read_typed(genesis_ref_state_id, ObjectType::RefState)?
        .ok_or("expected genesis RefState to exist")?;
    let genesis_payload = RefStatePayload::decode_canonical(
        &genesis_envelope.canonical_payload,
        genesis_envelope.schema_version,
    )?;

    // Otherwise identical to the real genesis RefState -- only `update_seq` and
    // `previous_ref_state_id` differ -- so this is exactly the shape `seal` itself writes, not a
    // contrived malformed object.
    let overflowed_payload = RefStatePayload {
        update_seq: u64::MAX,
        previous_ref_state_id: Some(genesis_ref_state_id),
        ..genesis_payload
    };
    let mut overflowed_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        overflowed_payload.to_canonical_bytes()?,
    );
    let overflowed_object_id = overflowed_envelope.object_id();
    overflowed_envelope.add_signature(maintainer_signature(
        &signer,
        ObjectType::RefState,
        overflowed_object_id,
    )?)?;
    let overflowed_object_id = object_store.write_object(&overflowed_envelope)?;
    prikk_store::force_ref_pointer_to_arbitrary_state_for_test_support(
        &layout,
        "heads/main",
        overflowed_object_id,
    )?;

    let output = prikk(&root)
        .env("PRIKK_MAINTAINER_KEY_ID", "aud08-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED),
        )
        .args(["seal", "--allow-no-audit"])
        .output()?;
    assert!(
        !output.status.success(),
        "seal must refuse an overflowing update_seq, got: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ref-state update_seq overflow"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("heads/main"), "stderr: {stderr}");

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
