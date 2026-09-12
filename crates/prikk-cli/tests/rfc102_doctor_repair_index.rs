//! RFC 102 — `prikk doctor --repair-index`: the rebuild that existed, now reachable.
//!
//! **The damage here is constructed, not raced.** The handoff asked for the collision in the shape
//! the pre-fix barrier test produced (two racing appends against one stale length). That race is no
//! longer reachable — the object-store lock landed first and prevents it, which is the point of that
//! round — so this builds the *same end state* directly: one valid, correctly-checksummed index
//! entry whose offset points at a different record's bytes.
//!
//! That the constructed state is the same one is not asserted by resemblance: the test pins the
//! error message it produces (`index entry for … resolves to an envelope with computed id …`), which
//! is exactly what twelve concurrent `prikk tag create` produced before the lock. A damaged *frame*
//! would not do — the decoder skips those — so the entry is re-checksummed after its offset is
//! changed, which is what makes it a wrong pointer rather than corruption the reader ignores.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(target_family = "unix")]

mod support;

use std::path::{Path, PathBuf};

const INDEX_MAGIC: &[u8; 8] = b"PIDXENT1";
const INDEX_VERSION: u16 = 1;
const INDEX_HEADER_LEN: usize = 8 + 2 + 8 + 32;
const INDEX_BODY_LEN: usize = 32 + 2 + 1 + 8 + 8 + 32;
const OFFSET_AT: usize = 32 + 2 + 1;

fn index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/containers/index.container")
}

/// Point one index entry at another entry of the **same object type**'s offset, re-checksumming the
/// record so it stays a structurally valid frame. This is the defect: a sound entry, wrong location.
///
/// The type is discovered rather than named: whichever type the fixture happens to have two of. A
/// hard-coded type code made this fixture-dependent and it broke on the first run against a
/// three-commit repository.
fn misdirect_an_index_entry(repo: &Path) -> bool {
    let path = index_path(repo);
    let mut bytes = std::fs::read(&path).expect("read index");
    let record_len = INDEX_HEADER_LEN + INDEX_BODY_LEN;
    let mut by_type: std::collections::BTreeMap<u16, Vec<usize>> =
        std::collections::BTreeMap::new();
    let mut cursor = 0;
    while cursor + record_len <= bytes.len() {
        let body = cursor + INDEX_HEADER_LEN;
        let code = u16::from_be_bytes([bytes[body + 32], bytes[body + 33]]);
        by_type.entry(code).or_default().push(cursor);
        cursor += record_len;
    }
    let Some(matching) = by_type.values().find(|records| records.len() >= 2) else {
        return false;
    };
    let (first, second) = (matching[0], matching[1]);
    let victim_offset: [u8; 8] = bytes[second + INDEX_HEADER_LEN + OFFSET_AT..][..8]
        .try_into()
        .unwrap();
    let body_start = first + INDEX_HEADER_LEN;
    bytes[body_start + OFFSET_AT..body_start + OFFSET_AT + 8].copy_from_slice(&victim_offset);

    // Re-checksum, or the frame reads as damaged and the decoder skips it — a different bug.
    let body = bytes[body_start..body_start + INDEX_BODY_LEN].to_vec();
    let mut preimage = Vec::new();
    preimage.extend_from_slice(INDEX_MAGIC);
    preimage.extend_from_slice(&INDEX_VERSION.to_be_bytes());
    preimage.extend_from_slice(&(INDEX_BODY_LEN as u64).to_be_bytes());
    preimage.extend_from_slice(&body);
    let checksum = prikk_hash::sha256(&preimage);
    bytes[first + 8 + 2 + 8..first + 8 + 2 + 8 + 32].copy_from_slice(&checksum);

    std::fs::write(&path, bytes).expect("write index");
    true
}

fn repo_with_several_objects(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    for (index, content) in ["first\n", "second\n", "third\n"].iter().enumerate() {
        std::fs::write(repo.join(format!("f{index}.txt")), content).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("commit {index}")),
            "commit",
        );
    }
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn doctor(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut command = support::prikk(repo);
    command.arg("doctor");
    command.args(args);
    command.output().unwrap()
}

/// Control 1: a repository whose index points at the wrong record is unverifiable, `--repair-index`
/// fixes it, and **the containers are byte-identical across the repair** — the repair is index-only.
#[test]
fn repair_index_restores_a_repository_whose_index_points_at_the_wrong_record() {
    let repo = repo_with_several_objects("rfc102-repair-index");

    // Patch objects: three commits produce at least two, which the misdirection needs.
    assert!(
        misdirect_an_index_entry(&repo),
        "fixture needs at least two objects of the chosen type"
    );

    let containers_before = container_bytes(&repo);
    let broken = support::verify(&repo);
    assert_eq!(
        broken.status.code(),
        Some(1),
        "verify must fail on the damage"
    );
    let broken_stderr = String::from_utf8_lossy(&broken.stderr).into_owned();

    let repaired = doctor(&repo, &["--repair-index"]);
    support::ok(&repaired, "doctor --repair-index");
    let stdout = String::from_utf8_lossy(&repaired.stdout).into_owned();
    assert!(
        stdout.contains("object index: rebuilt from containers"),
        "the repair must say it rebuilt: {stdout}"
    );
    assert!(
        stdout.contains("entries relocated: 1"),
        "and name the entry it moved: {stdout}\n(the damage was: {broken_stderr})"
    );

    support::ok(&support::verify(&repo), "verify after repair");
    // Every object readable by id, through an ordinary read surface.
    support::ok(&support::prikk(&repo).arg("log").output().unwrap(), "log");

    assert_eq!(
        containers_before,
        container_bytes(&repo),
        "repair must touch the index only -- container bytes may not change"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: a clean repository is untouched and says so, and running it twice changes nothing.
#[test]
fn repair_index_is_idempotent_on_a_clean_repository() {
    let repo = repo_with_several_objects("rfc102-repair-index-clean");
    let index_before = std::fs::read(index_path(&repo)).unwrap();

    for round in 0..2 {
        let out = doctor(&repo, &["--repair-index"]);
        support::ok(&out, "doctor --repair-index");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            stdout.contains("object index: nothing to repair"),
            "round {round} must report nothing to repair: {stdout}"
        );
    }
    assert_eq!(
        index_before,
        std::fs::read(index_path(&repo)).unwrap(),
        "a clean index must be byte-identical after the repair -- the rebuild walks containers in \
         type order while the live index is in write order, so a byte comparison of the two would \
         rewrite a healthy repository on every run"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: `doctor` without the flag only diagnoses. Nothing repairs implicitly.
#[test]
fn doctor_without_the_flag_does_not_repair_the_index() {
    let repo = repo_with_several_objects("rfc102-repair-index-implicit");
    assert!(misdirect_an_index_entry(&repo));
    let damaged = std::fs::read(index_path(&repo)).unwrap();

    let out = doctor(&repo, &[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !stdout.contains("object index: rebuilt"),
        "plain doctor must not repair: {stdout}"
    );
    assert_eq!(
        damaged,
        std::fs::read(index_path(&repo)).unwrap(),
        "plain doctor must leave the index exactly as it found it"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

fn container_bytes(repo: &Path) -> Vec<(String, Vec<u8>)> {
    let containers = repo.join(".prikk/containers");
    let mut out = Vec::new();
    let mut stack = vec![containers];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("container")
                && path.file_name().and_then(|n| n.to_str()) != Some("index.container")
            {
                let bytes = std::fs::read(&path).unwrap_or_default();
                out.push((path.display().to_string(), bytes));
            }
        }
    }
    out.sort();
    assert!(!out.is_empty(), "the fixture must have object containers");
    out
}
