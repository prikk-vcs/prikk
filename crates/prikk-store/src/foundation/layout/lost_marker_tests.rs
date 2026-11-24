//! 0.59.0 step K4: what `init` creates is one list; a `.prikk` holding only that list is a killed `init`'s; `init` never labels anything else; and the
//! sentence every command prints for a lost marker names a command that works.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use prikk_error::PrikkError;

use super::init_set::InitEntry;
use super::{CURRENT_FORMAT_VERSION, RepositoryLayout, SOUND_COPY};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
use crate::test_gates::test_support::unique_temp_dir;

fn prikk(root: &Path) -> PathBuf {
    root.join(".prikk")
}

/// Every entry under `.prikk`, relative, with a file's bytes (`None` for a directory).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(relative, None);
                walk(base, &path, out);
            } else {
                out.insert(relative, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    let base = prikk(root);
    walk(&base, &base, &mut out);
    out
}

/// A repository `init` finished, then its marker removed: exactly what a killed `init` leaves before the marker.
fn killed_before_the_marker(tag: &str) -> (PathBuf, RepositoryLayout) {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::remove_file(prikk(&root).join("FORMAT")).unwrap();
    (root, layout)
}

fn asks(layout: &RepositoryLayout) -> bool {
    layout.holds_only_a_killed_inits_state().unwrap()
}

#[test]
fn the_list_is_exactly_what_init_leaves() {
    let root = unique_temp_dir("k4-list-is-init");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut disk = snapshot(&root);
    assert_eq!(
        disk.remove(Path::new("FORMAT")),
        Some(Some(CURRENT_FORMAT_VERSION.to_vec())),
        "init writes the marker last, and it is not in the list"
    );
    let mut listed = BTreeMap::new();
    for entry in layout.init_entries().unwrap() {
        match entry {
            InitEntry::Directory(path) => listed.insert(path, None),
            InitEntry::File { path, bytes } => listed.insert(path, Some(bytes.to_vec())),
        };
    }
    assert_eq!(disk, listed, "init creates the list, and nothing else");
    // The write state every later writer also ensures is in the list.
    std::fs::remove_dir_all(prikk(&root).join("recovery")).unwrap();
    std::fs::remove_file(prikk(&root).join("active/default/witness")).unwrap();
    layout.ensure_write_state().unwrap();
    assert_eq!(
        snapshot(&root).len(),
        disk.len() + 1,
        "only the marker is outside it"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_killed_inits_state_is_unpopulated_at_every_stage_of_the_list() {
    let (root, layout) = killed_before_the_marker("k4-unpopulated");
    assert!(asks(&layout), "init finished but for the marker");
    // Nothing yet created is fine too: remove entries from the end.
    let entries = layout.init_entries().unwrap();
    for entry in entries.iter().rev() {
        match entry {
            InitEntry::File { path, .. } => std::fs::remove_file(prikk(&root).join(path)).unwrap(),
            InitEntry::Directory(path) => {
                let _ = std::fs::remove_dir(prikk(&root).join(path));
            }
        }
        assert!(asks(&layout), "a shorter killed init is still unpopulated");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_byte_in_any_file_init_creates_makes_it_populated() {
    let (root, layout) = killed_before_the_marker("k4-byte-flips");
    for entry in layout.init_entries().unwrap() {
        let InitEntry::File { path, bytes } = entry else {
            continue;
        };
        let file = prikk(&root).join(&path);
        let original = std::fs::read(&file).unwrap();
        assert_eq!(original, bytes, "{}", path.display());
        // Other bytes, or the right bytes and one more: populated. The right bytes, or none: not.
        std::fs::write(&file, b"x").unwrap();
        assert!(!asks(&layout), "{} holding other bytes", path.display());
        let mut longer = bytes.to_vec();
        longer.push(b'x');
        std::fs::write(&file, &longer).unwrap();
        assert!(!asks(&layout), "{} holding one byte more", path.display());
        std::fs::write(&file, b"").unwrap();
        assert!(asks(&layout), "{} emptied", path.display());
        std::fs::write(&file, bytes).unwrap();
        assert!(asks(&layout), "{} restored", path.display());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn anything_init_did_not_create_makes_it_populated() {
    let (root, layout) = killed_before_the_marker("k4-extras-flip");
    let at = |relative: &str| prikk(&root).join(relative);
    // An unknown file at the top, inside a listed directory, and inside a listed directory's listed child.
    for relative in [
        "stray",
        "refs/stray",
        "containers/patch/stray",
        "active/default/lock",
    ] {
        std::fs::write(at(relative), b"").unwrap();
        assert!(!asks(&layout), "{relative}, empty, is still not init's");
        std::fs::remove_file(at(relative)).unwrap();
        assert!(asks(&layout), "{relative} removed");
    }
    // An unknown directory, empty, and a directory where a file belongs.
    std::fs::create_dir(at("objects")).unwrap();
    assert!(!asks(&layout), "an unknown empty directory");
    std::fs::remove_dir(at("objects")).unwrap();
    std::fs::remove_file(at("worktree.marker")).unwrap();
    std::fs::create_dir(at("worktree.marker")).unwrap();
    assert!(!asks(&layout), "a directory where a file belongs");
    std::fs::remove_dir(at("worktree.marker")).unwrap();
    std::fs::write(at("worktree.marker"), b"").unwrap();
    // The worktree baseline `prikk init` records after the marker is not what a killed `init` leaves.
    std::fs::write(at("worktree.baseline"), b"b").unwrap();
    assert!(!asks(&layout), "a baseline exists only after init finished");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_markers_own_temporary_is_init_s_and_nothing_else_named_like_it_is() {
    let (root, layout) = killed_before_the_marker("k4-temp");
    let temp = prikk(&root).join("FORMAT.tmp.1.abc");
    for (held, unpopulated) in [
        (&b""[..], true),
        (b"8", true),
        (b"8\n", true),
        (b"7", false),
        (b"8\nx", false),
    ] {
        std::fs::write(&temp, held).unwrap();
        assert_eq!(asks(&layout), unpopulated, "a temporary holding {held:?}");
    }
    std::fs::remove_file(&temp).unwrap();
    std::fs::create_dir(&temp).unwrap();
    assert!(!asks(&layout), "a directory named like it");
    let _ = std::fs::remove_dir_all(root);
}

fn populate(root: &Path) {
    std::fs::write(prikk(root).join("containers/patch/a.container"), b"bytes").unwrap();
}

#[test]
fn init_finishes_over_what_a_killed_init_left_and_refuses_over_anything_else() {
    // (a) nothing there.
    let root = unique_temp_dir("k4-states-a");
    RepositoryLayout::init(root.clone()).unwrap();
    assert_eq!(std::fs::read(prikk(&root).join("FORMAT")).unwrap(), b"8\n");
    // (b) a killed init: the marker never written.
    let (root_b, _) = killed_before_the_marker("k4-states-b");
    RepositoryLayout::init(root_b.clone()).unwrap();
    assert_eq!(
        std::fs::read(prikk(&root_b).join("FORMAT")).unwrap(),
        b"8\n"
    );
    // (d) a killed init of a release before K4: the marker created and empty.
    let (root_d, _) = killed_before_the_marker("k4-states-d");
    std::fs::write(prikk(&root_d).join("FORMAT"), b"").unwrap();
    RepositoryLayout::init(root_d.clone()).unwrap();
    assert_eq!(
        std::fs::read(prikk(&root_d).join("FORMAT")).unwrap(),
        b"8\n"
    );
    // (c) and (e): populated, the marker absent or empty. Refused, and nothing written.
    for (tag, empty_marker) in [("k4-states-c", false), ("k4-states-e", true)] {
        let (root_c, _) = killed_before_the_marker(tag);
        populate(&root_c);
        if empty_marker {
            std::fs::write(prikk(&root_c).join("FORMAT"), b"").unwrap();
        }
        let before = snapshot(&root_c);
        let refused = RepositoryLayout::init(root_c.clone());
        let Err(PrikkError::Integrity(sentence)) = refused else {
            panic!("{tag}: init must refuse a populated repository: {refused:?}");
        };
        assert!(sentence.contains("`prikk format upgrade`"), "{sentence}");
        assert!(sentence.contains(SOUND_COPY), "{sentence}");
        assert_eq!(snapshot(&root_c), before, "{tag}: nothing was written");
        let _ = std::fs::remove_dir_all(root_c);
    }
    for dir in [root, root_b, root_d] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn every_command_that_opens_a_layout_prints_the_sentence_of_the_state() {
    let (unpopulated, _) = killed_before_the_marker("k4-sentence-unpopulated");
    let refused = RepositoryLayout::open(unpopulated.clone());
    let Err(PrikkError::Precondition(sentence)) = refused else {
        panic!("{refused:?}");
    };
    assert!(sentence.contains("`prikk init` finishes it"), "{sentence}");
    assert!(sentence.contains("absent"), "{sentence}");
    std::fs::write(prikk(&unpopulated).join("FORMAT"), b"").unwrap();
    let refused = RepositoryLayout::open(unpopulated.clone());
    let Err(PrikkError::Precondition(sentence)) = refused else {
        panic!("{refused:?}");
    };
    assert!(sentence.contains("is empty"), "{sentence}");
    let (populated, _) = killed_before_the_marker("k4-sentence-populated");
    populate(&populated);
    let refused = RepositoryLayout::open(populated.clone());
    let Err(PrikkError::Integrity(sentence)) = refused else {
        panic!("{refused:?}");
    };
    assert!(
        sentence.contains("`prikk format upgrade` verifies"),
        "{sentence}"
    );
    assert!(
        sentence.contains("prikk before 0.57.0 does not open format 8"),
        "{sentence}"
    );
    assert!(sentence.contains(SOUND_COPY), "{sentence}");
    assert!(!sentence.contains("required root-relative"), "{sentence}");
    for dir in [unpopulated, populated] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_kill_in_the_marker_write_leaves_it_absent_and_init_finishes() {
    let (root, _) = killed_before_the_marker("k4-kill-in-marker");
    for point in [TestFailPoint::MutableFileSync, TestFailPoint::MutableRename] {
        fail_once_for_test(point);
        let torn = RepositoryLayout::init(root.clone());
        clear_failpoint_for_test();
        assert!(torn.is_err(), "{point:?}: the injected failure must fire");
        assert!(
            !prikk(&root).join("FORMAT").exists(),
            "{point:?}: a failed write leaves no marker, never a part of one"
        );
        // What the failure left, if anything, is the marker's own temporary, which the question accepts.
        let layout_probe = RepositoryLayout::init(root.clone()).unwrap();
        assert_eq!(std::fs::read(prikk(&root).join("FORMAT")).unwrap(), b"8\n");
        std::fs::remove_file(prikk(&root).join("FORMAT")).unwrap();
        drop(layout_probe);
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_temporary_a_kill_left_is_finished_over_and_kept() {
    let (root, _) = killed_before_the_marker("k4-temp-finished");
    let temp = prikk(&root).join("FORMAT.tmp.77.deadbeef");
    std::fs::write(&temp, b"8").unwrap();
    RepositoryLayout::init(root.clone()).unwrap();
    assert_eq!(std::fs::read(prikk(&root).join("FORMAT")).unwrap(), b"8\n");
    assert_eq!(
        std::fs::read(&temp).unwrap(),
        b"8",
        "the remnant stays, counted, not removed"
    );
    RepositoryLayout::init(root.clone()).unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn init_over_a_present_marker_keeps_it() {
    for marker in [&b"6\n"[..], b"7\n", b"8\n"] {
        let root = unique_temp_dir("k4-present");
        RepositoryLayout::init(root.clone()).unwrap();
        populate(&root);
        std::fs::write(prikk(&root).join("FORMAT"), marker).unwrap();
        RepositoryLayout::init(root.clone()).unwrap();
        assert_eq!(std::fs::read(prikk(&root).join("FORMAT")).unwrap(), marker);
        let _ = std::fs::remove_dir_all(root);
    }
}
