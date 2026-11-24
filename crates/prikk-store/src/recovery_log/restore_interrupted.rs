//! 0.59.0 K2: a restore recognises every state its own interrupted writes can leave, and finishes it; any other state refuses as it always did.
//! Each torn state is built directly (bytes written to the file, no ptrace), so these run on every OS. A kill at a syscall boundary cannot leave the
//! part-written states (a `write` is one syscall); a kill inside a `write` of more than a page, or a power loss, can, and the real-kill sweep (K1's
//! `--recovery-restore` of a `Remove` run, ordinal 34) reached the file that is created and not yet written.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::*;
use crate::test_gates::test_support::unique_temp_dir;

const REMOVED: &[u8] = b"PREFPTR1 the bytes a repair removed";

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    (layout, root)
}

fn file(root: &Path, source: &str) -> std::path::PathBuf {
    root.join(".prikk").join(source)
}

/// A `Remove` run for `refs/tmp/a.tmp`, whose file is gone.
fn removed_run(layout: &RepositoryLayout) -> String {
    let _run = begin_run();
    save_remove(layout, "refs/tmp/a.tmp", REMOVED, "test")
        .expect("save")
        .id
}

fn put(root: &Path, source: &str, bytes: &[u8]) {
    let path = file(root, source);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn read(root: &Path, source: &str) -> Vec<u8> {
    std::fs::read(file(root, source)).unwrap()
}

const CANDIDATE: &str = "refs/tmp/a.tmp";

#[test]
fn an_absent_file_is_recreated_whole() {
    let (layout, root) = repository("k2-remove-absent");
    let id = removed_run(&layout);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(plan.written, "{plan:?}");
    assert_eq!(read(&root, CANDIDATE), REMOVED);
    let _ = std::fs::remove_dir_all(root);
}

/// **The state K1's ordinal 34 reached:** the file was created and not written. A middle prefix is the same state after a short write.
#[test]
fn a_file_created_and_not_finished_is_written_whole() {
    for prefix in [0, 1, REMOVED.len() / 2, REMOVED.len() - 1] {
        let (layout, root) = repository("k2-remove-partial");
        let id = removed_run(&layout);
        put(&root, CANDIDATE, &REMOVED[..prefix]);
        let plan = restore(&layout, &id, true).unwrap();
        assert!(plan.can_restore() && !plan.written, "{prefix}: {plan:?}");
        assert!(
            plan.conditions
                .iter()
                .any(|c| c.holds && c.text.contains("as an interrupted restore leaves")),
            "{prefix}: {plan:?}"
        );
        assert_eq!(
            plan.would_write,
            REMOVED.len(),
            "the whole file is written again"
        );
        let plan = restore(&layout, &id, false).unwrap();
        assert!(plan.written, "{prefix}: {plan:?}");
        assert_eq!(read(&root, CANDIDATE), REMOVED, "{prefix}");
        // And run again, the step is done and nothing is written.
        assert!(
            restore(&layout, &id, false)
                .unwrap()
                .steps
                .iter()
                .all(|step| step.done)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn any_other_bytes_at_the_path_still_refuse_and_stay() {
    let mut others: Vec<Vec<u8>> = vec![
        b"someone else's file".to_vec(),
        REMOVED.to_vec().into_iter().rev().collect(),
        [REMOVED, b"!"].concat(), // longer, with the removed bytes as its prefix: not a strict prefix
    ];
    let mut changed = REMOVED[..REMOVED.len() / 2].to_vec();
    changed[3] ^= 0xFF; // the right length for a prefix, one byte wrong
    others.push(changed);
    for bytes in others {
        let (layout, root) = repository("k2-remove-other");
        let id = removed_run(&layout);
        put(&root, CANDIDATE, &bytes);
        let plan = restore(&layout, &id, false).unwrap();
        assert!(!plan.can_restore() && !plan.written, "{plan:?}");
        assert!(
            plan.conditions
                .iter()
                .any(|c| !c.holds && c.text.contains("exists again, with other bytes")),
            "{plan:?}"
        );
        assert_eq!(read(&root, CANDIDATE), bytes, "nothing was written");
        let _ = std::fs::remove_dir_all(root);
    }
}

// ---- `Cut`: the restore appends the removed bytes ------------------------------------------------------------------------------------------

const WHOLE: &[u8] = b"kept-prefix|the removed tail a repair cut off";
const CUT_AT: usize = 11; // "kept-prefix"
const SOURCE: &str = WORKTREE_RECORD_SOURCE;

/// A `Cut` run: the file held [`WHOLE`], the repair cut it to its first [`CUT_AT`] bytes.
fn cut_run(layout: &RepositoryLayout, root: &Path) -> String {
    put(root, SOURCE, WHOLE);
    let entry = entry_for(layout, SOURCE, WHOLE, CUT_AT as u64, "test").expect("entry");
    let _run = begin_run();
    let id = append(layout.repository_mutation_root(), &entry)
        .expect("append")
        .id;
    put(root, SOURCE, &WHOLE[..CUT_AT]);
    id
}

#[test]
fn a_cut_whose_append_was_interrupted_is_finished() {
    for appended in 1..(WHOLE.len() - CUT_AT) {
        let (layout, root) = repository("k2-cut-partial");
        let id = cut_run(&layout, &root);
        put(&root, SOURCE, &WHOLE[..CUT_AT + appended]);
        let plan = restore(&layout, &id, true).unwrap();
        assert!(plan.can_restore(), "{appended}: {plan:?}");
        assert!(
            plan.conditions.iter().any(|c| c.holds
                && c.text.contains("as an interrupted restore leaves them; the rest is appended, and no byte there changes")),
            "{appended}: the plan says what was checked and the invariant: {plan:?}"
        );
        assert_eq!(
            plan.would_write,
            WHOLE.len() - CUT_AT - appended,
            "only the rest is appended: {appended}"
        );
        let plan = restore(&layout, &id, false).unwrap();
        assert!(plan.written, "{appended}: {plan:?}");
        assert_eq!(read(&root, SOURCE), WHOLE, "{appended}");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_cut_restored_whole_or_not_at_all_is_as_it_was() {
    let (layout, root) = repository("k2-cut-whole");
    let id = cut_run(&layout, &root);
    assert!(restore(&layout, &id, false).unwrap().written);
    assert_eq!(read(&root, SOURCE), WHOLE);
    assert!(
        restore(&layout, &id, false)
            .unwrap()
            .steps
            .iter()
            .all(|step| step.done)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_cut_with_a_changed_byte_still_refuses() {
    // The appended part is not a prefix of the removed bytes.
    let (layout, root) = repository("k2-cut-changed-tail");
    let id = cut_run(&layout, &root);
    let mut bytes = WHOLE[..CUT_AT + 5].to_vec();
    bytes[CUT_AT + 2] ^= 0xFF;
    put(&root, SOURCE, &bytes);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert_eq!(read(&root, SOURCE), bytes);
    // The bytes before the offset are not the ones the repair left.
    let (layout2, root2) = repository("k2-cut-changed-prefix");
    let id2 = cut_run(&layout2, &root2);
    let mut bytes = WHOLE[..CUT_AT + 5].to_vec();
    bytes[1] ^= 0xFF;
    put(&root2, SOURCE, &bytes);
    let plan = restore(&layout2, &id2, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert_eq!(read(&root2, SOURCE), bytes);
    // Longer than the whole restored file: not a restore's own state.
    let (layout3, root3) = repository("k2-cut-longer");
    let id3 = cut_run(&layout3, &root3);
    let longer = [WHOLE, b"later append"].concat();
    put(&root3, SOURCE, &longer);
    let plan = restore(&layout3, &id3, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert_eq!(read(&root3, SOURCE), longer);
    for r in [root, root2, root3] {
        let _ = std::fs::remove_dir_all(r);
    }
}

// ---- `Replace`: unchanged by this part; what must keep refusing ----------------------------------------------------------------------------

/// A `Replace` run whose saved bytes are a byte prefix of the newer file (a rebuild that appended).
fn replace_run(layout: &RepositoryLayout, root: &Path, previous: &[u8], written: &[u8]) -> String {
    put(root, SOURCE, written);
    let _run = begin_run();
    save_replace(layout, SOURCE, previous, written, "test")
        .expect("save")
        .id
}

#[test]
fn a_replace_restores_and_a_later_write_still_refuses_even_when_the_saved_bytes_are_a_prefix() {
    let previous: &[u8] = b"old bytes";
    let written: &[u8] = b"old bytes and what the repair appended";
    let (layout, root) = repository("k2-replace");
    let id = replace_run(&layout, &root, previous, written);
    assert!(restore(&layout, &id, false).unwrap().written);
    assert_eq!(read(&root, SOURCE), previous);
    let _ = std::fs::remove_dir_all(root);

    // **The shape a "first bytes are the removed bytes, and the file is longer" rule would have truncated:** a later ordinary append to the newer
    // file. The saved bytes are its prefix, so the first `len` bytes match; only the length (not recorded) tells it from an interrupted restore. It
    // must refuse.
    let (layout, root) = repository("k2-replace-later-append");
    let id = replace_run(&layout, &root, previous, written);
    let appended = [written, b" + a later ordinary write"].concat();
    put(&root, SOURCE, &appended);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert_eq!(read(&root, SOURCE), appended, "the later write is kept");
    let _ = std::fs::remove_dir_all(root);

    // A saved empty file is a prefix of everything: the same, and the worst case.
    let (layout, root) = repository("k2-replace-empty-previous");
    let id = replace_run(&layout, &root, b"", written);
    let appended = [written, b" + a later ordinary write"].concat();
    put(&root, SOURCE, &appended);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert_eq!(read(&root, SOURCE), appended);
    let _ = std::fs::remove_dir_all(root);
}

/// 0.59.0 K5: a `Replace` step whose file holds neither the saved bytes nor the written bytes says exactly that, not that the file "has changed since the
/// repair wrote its bytes" (it may never have finished writing them: a rewrite killed part-way leaves the new bytes up to some point and the old after).
#[test]
fn a_replace_whose_file_holds_neither_the_saved_nor_the_written_bytes_says_so() {
    let previous: &[u8] = b"the old bytes of the file";
    let written: &[u8] = b"NEW bytes, a different length!";
    let (layout, root) = repository("k5-replace-neither");
    let id = replace_run(&layout, &root, previous, written);
    // A rewrite killed part-way: the first bytes are the repair's, the rest are still the old file's.
    let torn = [&written[..7], &previous[7..]].concat();
    put(&root, SOURCE, &torn);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(!plan.can_restore() && !plan.written, "{plan:?}");
    assert!(
        plan.conditions.iter().any(|c| !c.holds
            && c.text
                .contains("holds neither the bytes the repair saved nor the bytes it wrote")),
        "{plan:?}"
    );
    assert!(
        !plan
            .conditions
            .iter()
            .any(|c| c.text.contains("has changed since the repair wrote")),
        "the sentence that claimed the repair wrote them is gone: {plan:?}"
    );
    assert_eq!(read(&root, SOURCE), torn, "nothing was written");
    // The two states the repair itself leaves are unchanged: the saved bytes (already restored) and the written bytes (restorable).
    put(&root, SOURCE, previous);
    let plan = restore(&layout, &id, false).unwrap();
    assert!(
        plan.conditions
            .iter()
            .all(|c| c.holds || c.text.contains("already")),
        "{plan:?}"
    );
    put(&root, SOURCE, written);
    assert!(restore(&layout, &id, false).unwrap().written);
    assert_eq!(read(&root, SOURCE), previous);
    let _ = std::fs::remove_dir_all(root);
}
