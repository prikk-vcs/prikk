//! RFC 102 measurement instrument: twelve concurrent `prikk tag create` against one repository.
//!
//! ```text
//! cargo test -p prikk --locked --test rfc102_object_store_lock_instrument -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d, in the shape `rfc133_node_count_memory.rs` and `dc59_commit_benchmark.rs` already
//! use: this is an instrument, not a correctness test. Its outcome is *timing-dependent by
//! construction* — that is the whole point of keeping it — so it must never gate a build.
//!
//! **Why it exists.** Before the object-store lock, this exact scenario left two index entries
//! claiming one offset and `prikk verify` failing. It reproduced in two runs of four on the
//! developer's machine and four of four on the reviewer's. My first pass ran it **once**, saw twenty
//! clean entries, and concluded separate processes "will not do it reliably" — a conclusion drawn
//! from a single sample, and wrong. A future reader should be able to run this rather than take
//! either of our words for it, which is why the instrument is committed and the anecdote is not.
//!
//! With the lock in place the expected outcome is: some writers refused with `lock conflict`, every
//! writer that succeeded readable, `verify` clean, and **no two index entries sharing an offset**.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(target_family = "unix")]

mod support;

use std::path::Path;

const RACERS: usize = 12;
const REPAIR_ROUNDS: usize = 40;

fn seal_a_block(repo: &Path) -> String {
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    support::ok(&support::commit(repo, "heads/main", "one"), "commit");
    let sealed = support::seal(repo, "heads/main");
    support::ok(&sealed, "seal");
    String::from_utf8_lossy(&sealed.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .expect("seal reports a block id")
        .trim()
        .to_string()
}

#[test]
#[ignore = "measurement instrument; timing-dependent by construction"]
fn twelve_concurrent_tag_creates_leave_a_verifiable_repository() {
    let repo = support::unique_repo("rfc102-object-store-lock");
    support::init(&repo);
    let block = seal_a_block(&repo);
    support::trust_maintainer(&repo);

    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..RACERS)
            .map(|index| {
                let repo = repo.clone();
                let block = block.clone();
                scope.spawn(move || {
                    let out = support::tag_create(&repo, &format!("tags/t{index}"), &block);
                    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                    (out.status.success(), stderr)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("racer"))
            .collect()
    });

    let succeeded = outcomes.iter().filter(|(ok, _)| *ok).count();
    let conflicts = outcomes
        .iter()
        .filter(|(ok, stderr)| !*ok && stderr.contains("lock conflict"))
        .count();
    let other: Vec<_> = outcomes
        .iter()
        .filter(|(ok, stderr)| !*ok && !stderr.contains("lock conflict"))
        .map(|(_, stderr)| stderr.trim().to_string())
        .collect();

    println!("racers={RACERS} succeeded={succeeded} lock-conflicts={conflicts}");
    for line in &other {
        println!("other failure: {line}");
    }

    let verify = support::verify(&repo);
    println!(
        "verify rc={:?}\n{}",
        verify.status.code(),
        String::from_utf8_lossy(&verify.stderr).trim()
    );

    // The instrument reports; it also asserts the two facts that would mean the lock is not working.
    assert!(
        succeeded >= 1,
        "at least one writer must get through: {outcomes:?}"
    );
    assert!(
        other.is_empty(),
        "every refusal must be a lock conflict, not corruption: {other:?}"
    );
    support::ok(&verify, "verify after the race");

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 102 v3 control (a): `doctor --repair-index` racing ordinary writers.
///
/// The repair is a writer to `index.container` like any other, and the first version of the verb
/// took no lock: forty repairs launched beside forty `tag create` runs left `verify` failing in four
/// rounds of six here — the repair scanned, a writer appended, and the repair installed a rebuilt
/// index that predated the append, discarding it.
///
/// `#[ignore]`d for the same reason as its sibling above: timing-dependent by construction. Its
/// deterministic counterpart is
/// `prikk-store`'s `a_repair_meeting_a_held_object_store_lock_is_refused_and_writes_nothing`, which
/// is the one that must never flake.
#[test]
#[ignore = "measurement instrument; timing-dependent by construction"]
fn repairs_racing_writers_leave_a_verifiable_repository() {
    let repo = support::unique_repo("rfc102-repair-race");
    support::init(&repo);
    let block = seal_a_block(&repo);
    support::trust_maintainer(&repo);

    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..REPAIR_ROUNDS {
            let repo_tag = repo.clone();
            let block = block.clone();
            handles.push(scope.spawn(move || {
                let out = support::tag_create(&repo_tag, &format!("tags/race{index}"), &block);
                (
                    "tag create",
                    out.status.success(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            }));
            let repo_repair = repo.clone();
            handles.push(scope.spawn(move || {
                let out = support::prikk(&repo_repair)
                    .args(["doctor", "--repair-index"])
                    .output()
                    .unwrap();
                (
                    "doctor --repair-index",
                    out.status.success(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("racer"))
            .collect()
    });

    let mut unexpected = Vec::new();
    for (what, ok, stderr) in &outcomes {
        if !*ok && !stderr.contains("container:object-store") {
            unexpected.push(format!("{what}: {}", stderr.trim()));
        }
    }
    let succeeded = outcomes.iter().filter(|(_, ok, _)| *ok).count();
    println!(
        "rounds={REPAIR_ROUNDS} invocations={} succeeded={succeeded} object-store-conflicts={}",
        outcomes.len(),
        outcomes.len() - succeeded - unexpected.len()
    );

    assert!(
        unexpected.is_empty(),
        "every failure must be an object-store lock conflict, not damage: {unexpected:#?}"
    );
    support::ok(
        &support::verify(&repo),
        "verify after repairs raced writers",
    );

    let _ = std::fs::remove_dir_all(&repo);
}
