//! Release-facing retired-format rejection proof: RFC 103 (format 1), RFC 102 Stage 3 (format 2,
//! design-v1.md §12.1), RFC 102 Stage 4 (format 3, applying the same already-ruled policy -- see
//! `layout.rs`'s own `RepositoryFormat` doc comment), RFC 102 Stage 5 (format 4, design-v1.md §14.7,
//! an owner decision this time rather than a reapplication), and RFC 102 Stage 6 (format 5,
//! design-v1.md §15.6, the same precedent again) each retire a repository format by refusing it at
//! open, not reading it in a bounded legacy mode -- there is no dual-path behavior left to exercise
//! across a command matrix, for any of the five. `build_legacy_fixture` remains load-bearing here:
//! design-v1.md §5 acceptance criterion 2 requires the rejection proven against a real fixture, not a
//! hand-built one, for every retired format.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod format_transition_support;

use format_transition_support::{
    ActiveFixture, MAINTAINER_KEY_ID, MAINTAINER_SEED_HEX, StrictFailure,
    build_current_format_strict_wal_fixture, build_legacy_fixture,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn prikk(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prikk"));
    command.current_dir(root);
    command
}

fn run(root: &Path, args: &[&str]) -> TestResult<Output> {
    Ok(prikk(root).args(args).output()?)
}

/// DC-83: the nanosecond timestamp alone was the defect — two threads of this same test binary can
/// observe the identical value (confirmed empirically: a 64-thread synchronized-barrier stress test
/// against bare `SystemTime::now()` nanoseconds produced real collisions, not a theoretical risk).
/// Adding `process::id()` alone would not have closed it: every thread in this one binary shares one
/// PID, so it cannot distinguish two racing threads — only a per-process atomic counter can. Combines
/// the process id (matching `prikk-store::test_support::unique_temp_dir`'s shape, so a collision
/// against a *different* binary's temp directory is still avoided) with a `fetch_add` counter that is
/// unique for the life of this process regardless of clock resolution or thread scheduling.
fn unique_root() -> TestResult<PathBuf> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "prikk-format-transition-{}-{nonce}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

fn run_owned(root: &Path, args: &[String]) -> TestResult<Output> {
    Ok(prikk(root)
        .env("PRIKK_AUTHOR_KEY_ID", "legacy-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("3636363636363636363636363636363636363636363636363636363636363636"),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(MAINTAINER_SEED_HEX),
        )
        .args(args)
        .output()?)
}

fn snapshot_tree(root: &Path) -> TestResult<BTreeMap<PathBuf, Vec<u8>>> {
    fn walk(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) -> TestResult {
        let mut entries = std::fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root)?.to_path_buf();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                snapshot.insert(relative.clone(), b"directory".to_vec());
                walk(root, &path, snapshot)?;
            } else if metadata.is_file() {
                snapshot.insert(relative, std::fs::read(path)?);
            } else {
                snapshot.insert(
                    relative,
                    std::fs::read_link(path)?
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            }
        }
        Ok(())
    }

    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot)?;
    Ok(snapshot)
}

fn assert_rejection_contract(
    args: &[&str],
    output: &Output,
    detected_format: &str,
    version_claim: Option<&str>,
    no_migration_claim: &str,
) {
    assert!(
        !output.status.success(),
        "{args:?} unexpectedly succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // `version_claim` is `None` for a format never itself the subject of a tagged release (formats 3,
    // 4, and 5, RFC 102 Stage 4's/Stage 5's/Stage 6's own bump targets) -- their rejection messages
    // name no specific "removed after X.Y.Z" version, since none could be verified from the release
    // record (`layout.rs`'s `LEGACY_FORMAT_3_VERSION`/`LEGACY_FORMAT_4_VERSION`/
    // `LEGACY_FORMAT_5_VERSION` arm doc comments).
    //
    // RFC 114 §5.3: the message states no-longer-supported plainly and offers no migration step --
    // `bundle export`/`bundle import` are no longer named here (that path was retired by this RFC;
    // see `layout.rs`'s five retired-format match arms). `no_migration_claim` differs by group:
    // formats 1-2 genuinely had a reader once (0.19.0) and say so was withdrawn, not absent; formats
    // 3-5 never shipped in any release and say so plainly.
    for expected in [detected_format, "requires format 6", no_migration_claim]
        .into_iter()
        .chain(version_claim)
    {
        assert!(
            stderr.contains(expected),
            "{args:?}: rejection message missing {expected:?}: {stderr}"
        );
    }
    for unexpected in ["bundle export", "bundle import"] {
        assert!(
            !stderr.contains(unexpected),
            "{args:?}: rejection message offers a migration step ({unexpected:?}) RFC 114 §5.3 \
             says must not be offered: {stderr}"
        );
    }
}

/// RFC 103 §4/design-v1.md §2 (format 1), RFC 102 Stage 3, design-v1.md §12.1 (format 2), RFC 102
/// Stage 4 (format 3), RFC 102 Stage 5, design-v1.md §14.7 (format 4), RFC 102 Stage 6,
/// design-v1.md §15.6 (format 5, the same proof re-run one format later again), and RFC 114 §5.3
/// (the message no longer offers a bundle export/import remedy -- that path was withdrawn, see
/// `layout.rs`'s five retired-format match arms): a retired-format repository is rejected at
/// `RepositoryLayout::open`, with a message naming the detected format, the required format, and a
/// plain no-migration statement (formats 1 and 2 state withdrawal -- 0.19.0 genuinely read them
/// once; formats 3, 4, and 5 state they were never in a released version, since none could be
/// verified from the release record -- see `layout.rs`'s `LEGACY_FORMAT_3_VERSION`/
/// `LEGACY_FORMAT_4_VERSION`/`LEGACY_FORMAT_5_VERSION` arms) — for every command, not a
/// command-specific subset, since rejection now happens before any command-specific logic runs.
/// Proven against real fixtures (`build_legacy_fixture`, differing only in what kind of
/// active-session state they carry and which format byte was flipped), not hand-built ones.
#[test]
fn retired_format_repository_is_rejected_at_open_for_every_command() -> TestResult {
    for (target_format, detected_format, version_claim, no_migration_claim) in [
        (
            b"1\n".as_slice(),
            "this repository uses format 1",
            Some("removed after 0.19.0"),
            "migration from format 1 is not supported",
        ),
        (
            b"2\n".as_slice(),
            "this repository uses format 2",
            Some("removed after 0.19.0"),
            "migration from format 2 is not supported",
        ),
        (
            b"3\n".as_slice(),
            "this repository uses format 3",
            None,
            "there is no supported migration path",
        ),
        (
            b"4\n".as_slice(),
            "this repository uses format 4",
            None,
            "there is no supported migration path",
        ),
        (
            b"5\n".as_slice(),
            "this repository uses format 5",
            None,
            "there is no supported migration path",
        ),
    ] {
        for active in [
            ActiveFixture::RollbackDraft,
            ActiveFixture::InterruptedPublication,
        ] {
            let root = unique_root()?;
            build_legacy_fixture(&root, active, target_format)?;
            let before = snapshot_tree(&root)?;

            for args in [
                vec!["status"],
                vec!["log"],
                vec!["worktree-status"],
                vec!["verify"],
                vec!["doctor"],
                vec!["checkout", "--plan-only"],
                vec!["rollback-preview"],
                vec!["commit", "-m", "must refuse"],
                vec!["seal", "--allow-no-audit"],
                vec![
                    "trust",
                    "maintainer",
                    "add",
                    "--key-id",
                    "legacy-refused",
                    "--public-key",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ],
            ] {
                // `run_owned`, not `run`: `seal` in particular constructs its signer from these env
                // vars before it ever opens the repository, so without them it fails at signer
                // construction and never reaches (or proves) the rejection this test is checking.
                let owned_args = args.iter().map(ToString::to_string).collect::<Vec<_>>();
                let output = run_owned(&root, &owned_args)?;
                assert_rejection_contract(
                    &args,
                    &output,
                    detected_format,
                    version_claim,
                    no_migration_claim,
                );
                assert_eq!(
                    snapshot_tree(&root)?,
                    before,
                    "{args:?} must not mutate a rejected repository"
                );
            }

            let _ = std::fs::remove_dir_all(root);
        }
    }
    Ok(())
}

/// `RepositoryLayout::init` refuses to initialize over an existing non-format-6 repository through
/// its own, separate check (`layout.rs::init`, not `read_repository_format`) — out of RFC 103's scope
/// since it never opens the repository for use, but still a real safety property worth keeping under
/// regression coverage: it must not silently reformat or clobber a format-1 repository in place.
#[test]
fn reinit_over_a_format1_repository_refuses_and_preserves_it() -> TestResult {
    let root = unique_root()?;
    build_legacy_fixture(&root, ActiveFixture::InterruptedPublication, b"1\n")?;
    let before = snapshot_tree(&root)?;

    let reinit = run(&root, &["init"])?;
    assert!(!reinit.status.success());
    assert_eq!(std::fs::read(root.join(".prikk/FORMAT"))?, b"1\n");
    assert_eq!(snapshot_tree(&root)?, before);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[path = "format_transition/matrix.rs"]
mod matrix;
