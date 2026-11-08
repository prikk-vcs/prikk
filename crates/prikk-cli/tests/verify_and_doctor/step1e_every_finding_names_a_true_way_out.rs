//! 0.57.0 step 1e: **every finding names a way out that is true**, and `verify` and `doctor` say it in
//! the same words.
//!
//! Each sentence (`prikk_store::WayOut::sentence`, the one function both readers call) is one of three
//! things. This file types them:
//! 1. **a command that clears the finding** -- the damage is made, the named command is typed, and
//!    `verify` exits 0 afterwards (acknowledged damage, loss and an unexplained tail →
//!    `--discard-damaged-commits`; missing, malformed or mismatched queue metadata →
//!    `--restore-queue-target`; a damaged ref pointer index → `--rebuild-pointer-index`);
//! 2. **the sound-copy sequence, in full** -- the sentence's own commands (`prikk setup <new-dir>`, then in
//!    it `prikk archive verify <backup>` and `prikk archive import <backup> --adopt`) are typed against a
//!    backup made before the damage, and the new repository verifies clean and holds the history;
//! 3. **the honest sentence that no command repairs it in place**, for damage where the commands that
//!    look relevant are shown to refuse (`--rebuild-pointer-index` over a damaged ref log,
//!    `--discard-damaged-commits` over an earlier damaged record, `--repair-tails` over interior damage).
//!
//! Every case also checks that `verify` prints the sentence (`way out [<finding>]: …`) and that `doctor`
//! carries the same text for the same finding.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use prikk_store::WayOut;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One installation: a config home shared by every repository of this test, so `setup` in a second
/// directory adopts the same maintainer key, as it does for a user restoring on their own machine.
struct Installation {
    home: PathBuf,
}

impl Installation {
    fn new(tag: &str) -> Self {
        let home = support::unique_repo(&format!("step1e-{tag}-home"));
        Self { home }
    }

    fn run(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prikk"));
        command
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("APPDATA", &self.home)
            .env_remove("PRIKK_AUTHOR_SEED")
            .env_remove("PRIKK_MAINTAINER_SEED")
            .env_remove("PRIKK_AUTHOR_SEED_FILE")
            .env_remove("PRIKK_MAINTAINER_SEED_FILE")
            .env_remove("PRIKK_AUTHOR_KEY_ID")
            .env_remove("PRIKK_MAINTAINER_KEY_ID");
        command
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.run(cwd).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// A repository with one sealed generation and two queued commits (the shape every WAL case needs).
    fn repo_with_queue(&self, tag: &str) -> PathBuf {
        let repo = support::unique_repo(&format!("step1e-{tag}"));
        let parent = repo.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(&repo);
        self.ok(&parent, &["setup", repo.to_str().unwrap()]);
        std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
        self.ok(&repo, &["commit", "-m", "one"]);
        self.ok(&repo, &["seal", "--allow-no-audit"]);
        std::fs::write(repo.join("b.txt"), "bravo\n").unwrap();
        self.ok(&repo, &["commit", "-m", "two"]);
        std::fs::write(repo.join("c.txt"), "charlie\n").unwrap();
        self.ok(&repo, &["commit", "-m", "three"]);
        repo
    }

    fn text(&self, cwd: &Path, args: &[&str]) -> (Option<i32>, String) {
        let output = self.run(cwd).args(args).output().unwrap();
        (
            output.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }
}

fn flip(repo: &Path, relative: &str, at: impl Fn(usize) -> usize) {
    let path = repo.join(".prikk").join(relative);
    let mut bytes = std::fs::read(&path).unwrap();
    let index = at(bytes.len());
    bytes[index] ^= 0xFF;
    std::fs::write(path, bytes).unwrap();
}

enum Cure {
    /// This command clears the finding: `verify` exits 0 afterwards.
    Clears(&'static [&'static str]),
    /// The command that looks relevant refuses (exits non-zero) and `verify` still fails.
    Refuses(&'static [&'static str]),
    /// No command is named; the sentence says so and gives the sound-copy sequence.
    None,
}

struct Case {
    name: &'static str,
    damage: fn(&Path),
    kind: WayOut,
    doctor_code: &'static str,
    cure: Cure,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "an acknowledged commit with a damaged body",
            damage: |repo| flip(repo, "active/default/queue.wal", |len| len - 40),
            kind: WayOut::AcknowledgedCommit,
            doctor_code: "PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE",
            cure: Cure::Clears(&["doctor", "--discard-damaged-commits"]),
        },
        Case {
            name: "the queue cut short (an acknowledged commit gone or cut)",
            damage: |repo| {
                let path = repo.join(".prikk/active/default/queue.wal");
                let bytes = std::fs::read(&path).unwrap();
                std::fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
            },
            kind: WayOut::AcknowledgedCommit,
            // Where the cut falls decides loss or damage; both are acknowledged commits, same way out.
            doctor_code: "PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-",
            cure: Cure::Clears(&["doctor", "--discard-damaged-commits"]),
        },
        Case {
            name: "an unexplained tail over an unreadable acknowledgment history",
            damage: |repo| {
                flip(repo, "active/default/witness", |len| len / 2);
                let path = repo.join(".prikk/active/default/queue.wal");
                let mut bytes = std::fs::read(&path).unwrap();
                bytes.extend([1u8, 2, 3]);
                std::fs::write(path, bytes).unwrap();
            },
            kind: WayOut::AcknowledgedCommit,
            doctor_code: "PRIKK-DOCTOR-COMMIT-WITNESS-UNKNOWN",
            cure: Cure::Clears(&["doctor", "--discard-damaged-commits"]),
        },
        Case {
            name: "queue metadata missing",
            damage: |repo| {
                std::fs::remove_file(repo.join(".prikk/active/default/ref-name")).unwrap()
            },
            kind: WayOut::ActiveMetadata,
            doctor_code: "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING",
            cure: Cure::Clears(&["doctor", "--restore-queue-target", "--ref", "heads/main"]),
        },
        Case {
            name: "queue metadata malformed",
            damage: |repo| {
                std::fs::write(
                    repo.join(".prikk/active/default/ref-name"),
                    b"not a ref!!\n",
                )
                .unwrap();
            },
            kind: WayOut::ActiveMetadata,
            doctor_code: "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED",
            cure: Cure::Clears(&["doctor", "--restore-queue-target", "--ref", "heads/main"]),
        },
        Case {
            name: "queue metadata naming an owner the acknowledgment history does not recognize",
            damage: |repo| {
                std::fs::write(repo.join(".prikk/active/default/ref-name"), b"heads/other")
                    .unwrap();
            },
            kind: WayOut::OwnershipMismatch,
            doctor_code: "PRIKK-DOCTOR-COMMIT-WITNESS-OWNERSHIP-MISSING",
            cure: Cure::Clears(&["doctor", "--restore-queue-target", "--ref", "heads/main"]),
        },
        Case {
            name: "a damaged ref pointer index (a stage cannot complete)",
            damage: |repo| {
                flip(repo, "refs/containers/pointer-index-a.container", |len| {
                    len / 2
                })
            },
            kind: WayOut::StageIncomplete,
            doctor_code: "PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE",
            cure: Cure::Clears(&["doctor", "--rebuild-pointer-index"]),
        },
        Case {
            name: "a damaged ref log",
            damage: |repo| flip(repo, "refs/containers/log-a.container", |len| len / 2),
            kind: WayOut::RefFile,
            doctor_code: "PRIKK-DOCTOR-VERIFY-REF-FILE-INCOMPLETE",
            cure: Cure::Refuses(&["doctor", "--rebuild-pointer-index"]),
        },
        Case {
            name: "a damaged block container",
            damage: |repo| flip(repo, "containers/block/a.container", |len| len / 2),
            kind: WayOut::StoredObject,
            doctor_code: "PRIKK-DOCTOR-VERIFY-OBJECT-INCOMPLETE",
            cure: Cure::None,
        },
        Case {
            name: "a damaged blob container",
            damage: |repo| flip(repo, "containers/blob/a.container", |len| len / 2),
            kind: WayOut::StoredObject,
            doctor_code: "PRIKK-DOCTOR-VERIFY-OBJECT-INCOMPLETE",
            cure: Cure::None,
        },
        Case {
            name: "an earlier queued record damaged",
            damage: |repo| flip(repo, "active/default/queue.wal", |_| 60),
            kind: WayOut::WalRecord,
            doctor_code: "PRIKK-DOCTOR-VERIFY-WAL-RECORD-INCOMPLETE",
            cure: Cure::Refuses(&["doctor", "--discard-damaged-commits", "--plan-only"]),
        },
        Case {
            name: "interior damage in an appended file (author keys)",
            damage: |repo| flip(repo, "trust/author-keys.container", |len| len / 2),
            kind: WayOut::AppendedFileInterior,
            doctor_code: "PRIKK-DOCTOR-APPENDED-FILE-INTERIOR-DAMAGE",
            cure: Cure::Refuses(&["doctor", "--repair-tails"]),
        },
    ]
}

/// **Every case: verify and doctor print the same sentence, and the sentence is true.**
///
/// **Perturb (red controls):** make a sentence name a command that does not clear its finding (the 1a
/// ordering row's first draft named `--discard-damaged-commits`) -- the case whose `Clears`/`Refuses`
/// leg types that command goes red; make `verify` stop printing the way-out block, or `doctor` stop using
/// the shared function for a kind -- the "same sentence" assertions go red.
#[test]
fn every_failing_finding_names_a_way_out_that_is_true_and_the_two_readers_agree() {
    let env = Installation::new("cases");
    for case in cases() {
        let repo = env.repo_with_queue("case");
        (case.damage)(&repo);

        let (code, verify) = env.text(&repo, &["verify"]);
        assert_eq!(code, Some(1), "{}: verify fails\n{verify}", case.name);
        let sentence = case.kind.sentence();
        assert!(
            verify.contains("way out ["),
            "{}: verify prints a way out\n{verify}",
            case.name
        );
        assert!(
            verify.contains(&sentence),
            "{}: verify prints the shared sentence\n{verify}",
            case.name
        );
        let (code, doctor) = env.text(&repo, &["doctor"]);
        assert_eq!(code, Some(1), "{}: doctor fails\n{doctor}", case.name);
        assert!(doctor.contains(case.doctor_code), "{}: {doctor}", case.name);
        // `doctor` also prints `verify`'s report, which carries the sentence; the recommendation of the
        // issue itself is what must say it.
        assert!(
            doctor
                .lines()
                .any(|line| line.starts_with("  recommendation: ") && line.contains(&sentence)),
            "{}: a doctor recommendation carries the same sentence\n{doctor}",
            case.name
        );

        // The command the sentence names is the one typed below, not a different one.
        if let Cure::Clears(args) | Cure::Refuses(args) = &case.cure {
            let named = format!("prikk {} {}", args[0], args[1]);
            assert!(
                sentence.contains(&named),
                "{}: the sentence names `{named}`\n{sentence}",
                case.name
            );
        }
        match case.cure {
            Cure::Clears(args) => {
                let (code, out) = env.text(&repo, args);
                assert_eq!(code, Some(0), "{}: {args:?}\n{out}", case.name);
                let (code, verify) = env.text(&repo, &["verify"]);
                assert_eq!(
                    code,
                    Some(0),
                    "{}: verify is clean after {args:?}\n{verify}",
                    case.name
                );
            }
            Cure::Refuses(args) => {
                let (code, out) = env.text(&repo, args);
                assert_ne!(code, Some(0), "{}: {args:?} must refuse\n{out}", case.name);
                let (code, _) = env.text(&repo, &["verify"]);
                assert_eq!(code, Some(1), "{}: verify still fails", case.name);
                assert!(
                    sentence.contains("no command repairs") || sentence.contains("no command"),
                    "{}: the honest sentence says no command repairs it: {sentence}",
                    case.name
                );
                assert!(sentence.contains("prikk setup <new-dir>"), "{}", case.name);
            }
            Cure::None => {
                assert!(
                    sentence.contains("no command repairs"),
                    "{}: {sentence}",
                    case.name
                );
                assert!(sentence.contains("prikk setup <new-dir>"), "{}", case.name);
            }
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// **The sound-copy sequence, typed once, literally as the sentence gives it.** A backup is made while
/// the repository is sound; the repository's block container is then damaged (nothing repairs that in
/// place); the sentence's commands are typed in a new directory -- `prikk setup <new-dir>`, then in it
/// `prikk archive verify <backup>` and `prikk archive import <backup> --adopt` -- and the new repository
/// verifies clean, holds the sealed history, and has its files.
///
/// **Perturb (red control):** drop `setup` from the sequence (the 1a text named a "fresh directory" but
/// not the `setup` that makes it a repository) -- the import into a plain directory refuses and this goes red.
#[test]
fn the_sound_copy_sequence_typed_as_the_sentence_gives_it_leaves_a_clean_repository() {
    let env = Installation::new("sound-copy");
    let source = env.repo_with_queue("sound-source");
    env.ok(&source, &["seal", "--allow-no-audit"]); // seal the queued work too: the backup carries sealed history
    let backup = std::env::temp_dir().join(format!(
        "prikk-step1e-backup-{}.archive",
        support::unique_suffix()
    ));
    env.ok(&source, &["archive", "export", backup.to_str().unwrap()]);

    flip(&source, "containers/block/a.container", |len| len / 2);
    let (code, verify) = env.text(&source, &["verify"]);
    assert_eq!(code, Some(1), "{verify}");
    let sentence = WayOut::StoredObject.sentence();
    assert!(verify.contains(&sentence), "{verify}");

    // The sentence's own commands, in its own order.
    for typed in [
        "prikk setup <new-dir>",
        "prikk archive verify <backup>",
        "prikk archive import <backup> --adopt",
    ] {
        assert!(
            prikk_store::SOUND_COPY.contains(typed),
            "the sentence gives `{typed}` literally"
        );
    }
    let new_dir = support::unique_repo("step1e-new-dir");
    let _ = std::fs::remove_dir_all(&new_dir);
    env.ok(
        source.parent().unwrap(),
        &["setup", new_dir.to_str().unwrap()],
    );
    let verified = env.ok(&new_dir, &["archive", "verify", backup.to_str().unwrap()]);
    assert!(
        String::from_utf8_lossy(&verified.stdout).contains("(adopted)"),
        "archive verify reports the sealing key as adopted in <new-dir>: {}",
        String::from_utf8_lossy(&verified.stdout)
    );
    env.ok(
        &new_dir,
        &["archive", "import", backup.to_str().unwrap(), "--adopt"],
    );

    let (code, verify) = env.text(&new_dir, &["verify"]);
    assert_eq!(code, Some(0), "the new repository verifies clean\n{verify}");
    let (_, log) = env.text(
        &new_dir,
        &["log", "--ref", "heads/main", "--format", "json"],
    );
    assert_eq!(
        log.matches("\"block_id\"").count(),
        2,
        "both sealed blocks arrived\n{log}"
    );
    for file in ["a.txt", "b.txt", "c.txt"] {
        assert!(
            new_dir.join(file).exists(),
            "{file} is in the new directory"
        );
    }

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&new_dir);
    let _ = std::fs::remove_file(backup);
}
