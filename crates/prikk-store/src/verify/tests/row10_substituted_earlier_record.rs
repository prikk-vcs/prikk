//! 0.50.0 step 1, A6 item 2 (019 §5.6, row 10 J): row 10's own text named no copy and said nothing
//! about the record this check itself already read as sound. This test proves the *data*
//! `print_verify_report` (prikk-cli, a separate crate, so its own printed string is not reachable
//! from here) formats: a real on-disk WAL, three real commits, record 2's own on-disk frame
//! substituted for record 3's envelope (re-encoded with a correct checksum for seq 2 -- a forged
//! but structurally sound record, not damage), reads `commit_witness_verdict == Healthy { last_seq:
//! 3 }` (the last record's own frame hash is untouched) alongside `commit_witness_running_hash_
//! agrees == Some(false)` (the chain behind it disagrees) -- exactly the two fields the fixed text
//! consumes: naming sequence 3 as "itself still sound" while refusing to trust the history behind
//! it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::commit_boundary::classification::Verdict;
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::{Wal, encode_record_for_test};
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions, verify_repository};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("a6-row10-substituted", &[0x4d_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "a6-item2",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
}

#[test]
fn verify_names_the_last_record_sound_and_the_history_behind_it_untrusted()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("a6-row10-substituted-earlier-record");
    let layout = RepositoryLayout::init(root)?;
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    commit(&layout, "c.txt", b"three");

    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let genuine = wal.replay()?;
    assert_eq!(genuine.records.len(), 3);

    // Re-encode every record fresh, with record 2's own envelope replaced by record 3's -- each
    // frame keeps its own real seq and gets a correct checksum for its (possibly substituted) body,
    // so every frame is individually sound; only the *chain* is now a forgery.
    let mut forged_bytes = Vec::new();
    for (index, record) in genuine.records.iter().enumerate() {
        let envelope = if index == 1 {
            genuine.records[2].envelope.clone()
        } else {
            record.envelope.clone()
        };
        let forged_record = crate::wal::WalRecord {
            seq: record.seq,
            envelope,
        };
        forged_bytes.extend(encode_record_for_test(&forged_record)?);
    }
    std::fs::write(wal.path(), &forged_bytes)?;

    let verification = verify_repository(&layout)?;
    assert_eq!(
        verification.commit_witness_verdict,
        Some(Verdict::Healthy { last_seq: 3 }),
        "the last record's own frame hash is untouched -- D3 alone cannot see the substitution"
    );
    assert_eq!(
        verification.commit_witness_running_hash_agrees,
        Some(false),
        "row 10: the chain behind the last record disagrees with the witness's own running hash"
    );
    Ok(())
}
